//! `k5 mail watch`: the mail notices (which inboxes wake an agent, the
//! VIPs, the check window, the digest, a pause and the log), as the shell
//! CLI's `cmd_mail_watch` does it (`cli/src/mail-watch.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `mail` is an `id` tool, so with `HOOK_SOCK` set: the stale-passport
//!    or require-passport lines, exit 3 ([`crate::prelude::id_tool`]).
//! 3. The words ([`parse`]): `--workspace` or `--project` with a value and
//!    `--json` are taken out wherever they are; the first word left is the
//!    sub-command. A mistake is its one line on stderr, exit 2, and
//!    nothing is sent. `digest instructions` prints its two lines and
//!    sends nothing.
//! 4. A POST's body is `_mw_body`'s ([`body`]): `key=value` words, each
//!    value read as JSON when it is JSON, printed by `json.dumps`. The
//!    workspace word rides unquoted (`${project:+project=$project}`), so
//!    a workspace path with white space in it is split into more words,
//!    and a word with no `=` stops `_mw_body`'s Python with a traceback:
//!    the body is then empty, and it is still sent.
//! 5. One `reply=$(cli_send_json GET|POST ...)` ([`crate::transport::Client::send_json`]):
//!    the cell socket first when there is a passport with a live socket,
//!    then TCP. `reply` is the answer, a newline and its status, after one
//!    `\n000` for each refused connect `_k2_curl` tried again; a TCP call
//!    that failed holds curl's `\n000` for each try and one more
//!    ([`reply`]). A POST that went out on the cell socket with no answer
//!    is the transport's line, exit 1.
//! 6. `_mw_python` on `reply` ([`reader`]): not JSON is `Error: ` and its
//!    first 200 characters, exit 1; a 2xx (but `"ok": false`) runs the
//!    sub-command's printer ([`Printer`]); anything else is the error's
//!    hint, message or code, exit 3 (owner only, forbidden, a 403), 2
//!    (usage) or 1.
//! 7. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): a
//! command line with `-h` or `--help` (the front sends those on,
//! [`crate::front`]); a word that is not UTF-8; a value flag (`--add`,
//! `--remove`, `--wake`, `--days`, `--tz`, `--for`, `--limit`) as the last
//! word (the shell stops on `$2: unbound variable`, a line with its
//! script's path in it: the plan's D6); a workspace path with `*`, `?` or
//! `[` in it where the shell expands it unquoted (a file name pattern);
//! a `--limit` that curl would not put in a URL as it is (a space, a
//! control byte or `#`); a `digest --every` value that is not ASCII
//! (Python's `\d` and `lower()` read all of Unicode); `HOOK_SOCK` set
//! with a `CODEX_` variable exported; a port that is set but is no port
//! curl takes; and a `python3` the binary can't tell.
//!
//! Where this differs from the shell:
//!
//! - An answer above the environment's limit for one string (128 KiB on
//!   Linux): `_mw_python` hands the answer to Python in an environment
//!   variable, so there `python3` can't start (`Argument list too long`,
//!   exit 126). The binary reads the answer and prints it.
//! - An answer that is not UTF-8 is read with U+FFFD for each bad byte
//!   (the shell's Python reads it through `surrogateescape`). A string
//!   with half of a surrogate pair prints as `\udXXX` where Python's
//!   `print` fails. The daemon sends neither.
//! - `int()` of a string reads ASCII digits only (Python's reads every
//!   Unicode digit).

use super::super::{curl_port, raised, s, Raise, Shown};
use super::py::{attr_get, big, concat, concat_error, iter, join, py_index, py_int, text};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{encode, Error, Response};
use k2_client::brand::CLI_NAME;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The settings and the status.
pub const WATCH: &str = "/cli/mail/watch";
/// `on`, `off`, the window, the digest, `outside-window`, `snippets`,
/// `vip-line`.
pub const SET: &str = "/cli/mail/watch/set";
/// `inboxes --add/--remove`.
pub const INBOXES: &str = "/cli/mail/watch/inboxes";
/// `vip add/rm`.
pub const VIP: &str = "/cli/mail/watch/vip";
/// `pause`.
pub const PAUSE: &str = "/cli/mail/watch/pause";
/// `pause --all`.
pub const PAUSE_ALL: &str = "/cli/mail/watch/pause-all";
/// `log`.
pub const LOG: &str = "/cli/mail/watch/log";

/// `digest instructions`' two lines.
pub const DIGEST_INSTRUCTIONS: &str = concat!(
    "The digest's extra instructions are its WAKEUP.md: .k2/heartbeats/mail-digest/WAKEUP.md\n",
    "Write it there. It's added under the list of new mail on each digest.\n",
);

/// `--weekdays`.
pub const WEEKDAYS: &str = "mon,tue,wed,thu,fri";

/// The line of `_mw_python`'s `python3 -c` that runs the printer.
pub const EXEC_LINE: u32 = 10;
/// The line of `_mw_python` that prints a refusal's hint.
pub const ERR_LINE: u32 = 15;
/// The line of `_mw_body`'s Python that splits a word at its `=`.
pub const BODY_SPLIT_LINE: u32 = 5;

/// What prints a 2xx answer (the source `_mw_python` runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Printer {
    /// `_mw_status_print`, with `--json` the answer itself.
    Status,
    /// `vip list`: the VIPs, one a line, or `No VIPs.`.
    VipList,
    /// `vip add|rm`: `VIPs: ...`.
    VipSet,
    /// `pause`: `Resumed.` or `Paused until ...`.
    Pause,
    /// `log`, with `--json` the answer itself.
    Log,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// These lines on stdout, exit 0; nothing is sent.
    Print(&'static str),
    /// This line on stderr, exit 2; nothing is sent.
    Usage(String),
    /// `cli_send_json GET <path> <query>`.
    Get {
        path: &'static str,
        query: String,
        printer: Printer,
    },
    /// `cli_send_json POST <path> "$(_mw_body <words>)"`.
    Post {
        path: &'static str,
        words: Vec<String>,
        printer: Printer,
    },
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub plan: Plan,
    /// `--json` (`MW_JSON=1`).
    pub json: bool,
}

/// `k5 mail watch ...`. `None`: the shell answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(2)
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
    let ask = parse(&words, &settings.project)?;
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, ask, hook_sock.as_deref(), settings))
}

fn run_with(args: &[OsString], ask: Ask, hook_sock: Option<&Path>, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, super::TOOL) {
        return stop;
    }
    let (post, path, payload, printer) = match ask.plan {
        Plan::Print(text) => {
            return match out::stdout(text) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Plan::Usage(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
        Plan::Get {
            path,
            query,
            printer,
        } => (false, path, query, printer),
        Plan::Post {
            path,
            words,
            printer,
        } => {
            let (payload, raise) = body(&words);
            if let Some(r) = raise {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), r.text().as_bytes());
            }
            (true, path, payload, printer)
        }
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let passport = client.passport().is_some();
    let result = client.send_json(post, path, &payload);
    let exit = match &result {
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e @ (Error::NoCellSocket { .. } | Error::NoCredential { .. })) => {
            out::stderr_line(&format!("Error: {e}"));
            Exit::Error
        }
        _ => {
            if let Ok(r) = &result {
                closing.answer(r, passport);
            }
            reader(
                &reply(&result, client.refused_before_answer()),
                printer,
                ask.json,
            )
            .print()
        }
    };
    closing.flush(&mut client, exit);
    exit
}

/// The words after `mail watch`, read as `cmd_mail_watch` reads them, with
/// `project` the shell's `$PROJECT`. `None`: the shell answers them.
pub fn parse(words: &[&str], project: &str) -> Option<Ask> {
    let mut chosen = "";
    let mut json = false;
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--workspace" | "--project" => {
                let Some(v) = words.get(i + 1) else {
                    return Some(usage(
                        json,
                        format!("Error: {} needs a workspace", words[i]),
                    ));
                };
                chosen = *v;
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            w => {
                rest.push(w);
                i += 1;
            }
        }
    }
    let project = if chosen.is_empty() { project } else { chosen };
    let sub = rest.first().copied().unwrap_or("");
    let rest: &[&str] = rest.get(1..).unwrap_or_default();
    let pj = if project.is_empty() {
        String::new()
    } else {
        format!("project={}", encode::quote(project))
    };
    let plan = match sub {
        "" | "status" => status(&pj),
        "on" | "off" => post(
            SET,
            unquoted(project)?,
            [format!("on={}", sub == "on")],
            Printer::Status,
        ),
        "inboxes" => inboxes(rest, project, &pj)?,
        "vip" => vip(rest, project, &pj)?,
        "window" => window(rest, project)?,
        "digest" => match rest.first().copied() {
            Some("instructions") => Plan::Print(DIGEST_INSTRUCTIONS),
            Some("--every") => {
                let v = rest.get(1).copied().unwrap_or("");
                if v.is_empty() {
                    Plan::Usage(
                        concat!(
                            "Error: usage: ",
                            cli_name!(),
                            " mail watch digest --every 60m"
                        )
                        .into(),
                    )
                } else {
                    if !v.is_ascii() {
                        return None;
                    }
                    match duration(v) {
                        Some(secs) => post(
                            SET,
                            unquoted(project)?,
                            [format!("digestEverySecs={secs}")],
                            Printer::Status,
                        ),
                        None => {
                            Plan::Usage(format!("Error: '{v}' is not a duration (use 60m, 2h)"))
                        }
                    }
                }
            }
            _ => Plan::Usage(
                concat!(
                    "Error: usage: ",
                    cli_name!(),
                    " mail watch digest --every 60m | digest instructions"
                )
                .into(),
            ),
        },
        "outside-window" => match rest.first().copied() {
            Some(w @ ("deliver" | "hold")) => post(
                SET,
                unquoted(project)?,
                [format!("vipOutsideWindow={}", w == "deliver")],
                Printer::Status,
            ),
            _ => Plan::Usage(
                concat!(
                    "Error: usage: ",
                    cli_name!(),
                    " mail watch outside-window deliver|hold"
                )
                .into(),
            ),
        },
        "snippets" | "vip-line" => match rest.first().copied() {
            Some(w @ ("on" | "off")) => {
                let key = if sub == "vip-line" {
                    "vipSessionLine"
                } else {
                    "snippets"
                };
                post(
                    SET,
                    unquoted(project)?,
                    [format!("{key}={}", w == "on")],
                    Printer::Status,
                )
            }
            _ => Plan::Usage(format!("Error: usage: {CLI_NAME} mail watch {sub} on|off")),
        },
        "pause" => pause(rest, project)?,
        "log" => log(rest, &pj)?,
        other => Plan::Usage(format!(
            "Unknown 'mail watch' subcommand: {other}. See {CLI_NAME} mail watch --help."
        )),
    };
    Some(Ask { plan, json })
}

fn usage(json: bool, line: String) -> Ask {
    Ask {
        plan: Plan::Usage(line),
        json,
    }
}

fn status(pj: &str) -> Plan {
    Plan::Get {
        path: WATCH,
        query: pj.to_string(),
        printer: Printer::Status,
    }
}

fn post<const N: usize>(
    path: &'static str,
    mut words: Vec<String>,
    more: [String; N],
    printer: Printer,
) -> Plan {
    words.extend(more);
    Plan::Post {
        path,
        words,
        printer,
    }
}

/// The words `${project:+project=$project}` gives, unquoted: none for no
/// workspace, else `project=<it>` split at runs of space, tab and newline.
/// `None` when it has a file name pattern character (the shell would
/// match it against files): the shell's.
pub fn unquoted(project: &str) -> Option<Vec<String>> {
    if project.is_empty() {
        return Some(Vec::new());
    }
    if project.contains(['*', '?', '[']) {
        return None;
    }
    Some(
        format!("project={project}")
            .split([' ', '\t', '\n'])
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// The value after a value flag, or `None` when it is the last word (the
/// shell's `$2: unbound variable`).
fn value<'a>(rest: &[&'a str], i: usize) -> Option<&'a str> {
    rest.get(i + 1).copied()
}

fn inboxes(rest: &[&str], project: &str, pj: &str) -> Option<Plan> {
    const USAGE: &str = concat!(
        "Error: usage: ",
        cli_name!(),
        " mail watch inboxes [--add <addr> [--wake vip|all]] [--remove <addr>]"
    );
    let (mut add, mut remove, mut wake) = ("", "", "");
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--add" => add = value(rest, i)?,
            "--remove" => remove = value(rest, i)?,
            "--wake" => wake = value(rest, i)?,
            _ => return Some(Plan::Usage(USAGE.into())),
        }
        i += 2;
    }
    if add.is_empty() && remove.is_empty() {
        return Some(status(pj));
    }
    if !wake.is_empty() && !matches!(wake, "vip" | "all") {
        return Some(Plan::Usage("Error: --wake is vip or all".into()));
    }
    let add_json = if add.is_empty() {
        "[]".to_string()
    } else {
        dumps(&Value::Array(vec![Value::Object(vec![
            (Text::from("address"), s(add)),
            (
                Text::from("wake"),
                s(if wake.is_empty() { "vip" } else { wake }),
            ),
        ])]))
    };
    let remove_json = if remove.is_empty() {
        "[]".to_string()
    } else {
        dumps(&Value::Array(vec![s(remove)]))
    };
    Some(post(
        INBOXES,
        unquoted(project)?,
        [format!("add={add_json}"), format!("remove={remove_json}")],
        Printer::Status,
    ))
}

fn vip(rest: &[&str], project: &str, pj: &str) -> Option<Plan> {
    let verb = rest
        .first()
        .copied()
        .filter(|w| !w.is_empty())
        .unwrap_or("list");
    let rest: &[&str] = rest.get(1..).unwrap_or_default();
    Some(match verb {
        "list" => Plan::Get {
            path: WATCH,
            query: pj.to_string(),
            printer: Printer::VipList,
        },
        "add" | "rm" | "remove" => {
            if rest.first().is_none_or(|w| w.is_empty()) {
                return Some(Plan::Usage(format!(
                    "Error: usage: {CLI_NAME} mail watch vip {verb} <addr|@domain>"
                )));
            }
            let list = dumps(&Value::Array(rest.iter().map(|w| s(w)).collect()));
            let key = if verb == "add" { "add" } else { "remove" };
            post(
                VIP,
                unquoted(project)?,
                [format!("{key}={list}")],
                Printer::VipSet,
            )
        }
        _ => Plan::Usage(
            concat!(
                "Error: usage: ",
                cli_name!(),
                " mail watch vip list | add <entry> | rm <entry>"
            )
            .into(),
        ),
    })
}

fn window(rest: &[&str], project: &str) -> Option<Plan> {
    let spec = rest.first().copied().unwrap_or("");
    if spec.is_empty() {
        return Some(Plan::Usage(
            concat!("Error: usage: ", cli_name!(), " mail watch window 09:00-17:00 [--weekdays|--days mon,tue] [--tz Area/City] | window off").into(),
        ));
    }
    let rest: &[&str] = rest.get(1..).unwrap_or_default();
    let (mut days, mut tz) = ("", "");
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--weekdays" => {
                days = WEEKDAYS;
                i += 1;
            }
            "--days" => {
                days = value(rest, i)?;
                i += 2;
            }
            "--tz" => {
                tz = value(rest, i)?;
                i += 2;
            }
            other => return Some(Plan::Usage(format!("Error: unknown flag: {other}"))),
        }
    }
    let win = if spec == "off" {
        r#"{"off":true}"#.to_string()
    } else {
        let Some((start, end)) = spec.split_once('-') else {
            return Some(Plan::Usage("Error: window is HH:MM-HH:MM, or off".into()));
        };
        let mut o = vec![(Text::from("start"), s(start)), (Text::from("end"), s(end))];
        let days: Vec<Value> = days.split(',').filter(|d| !d.is_empty()).map(s).collect();
        if !days.is_empty() {
            o.push((Text::from("days"), Value::Array(days)));
        }
        if !tz.is_empty() {
            o.push((Text::from("tz"), s(tz)));
        }
        dumps(&Value::Object(o))
    };
    Some(post(
        SET,
        unquoted(project)?,
        [format!("window={win}")],
        Printer::Status,
    ))
}

fn pause(rest: &[&str], project: &str) -> Option<Plan> {
    let (mut dur, mut resume, mut all) = ("", false, false);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--for" => {
                dur = value(rest, i)?;
                i += 2;
            }
            "--resume" => {
                resume = true;
                i += 1;
            }
            "--all" => {
                all = true;
                i += 1;
            }
            _ => {
                return Some(Plan::Usage(
                    concat!(
                        "Error: usage: ",
                        cli_name!(),
                        " mail watch pause [--for 2h | --resume]"
                    )
                    .into(),
                ))
            }
        }
    }
    // These words are quoted ("${pause_pairs[@]}"): never split.
    let mut words = Vec::new();
    if !project.is_empty() {
        words.push(format!("project={project}"));
    }
    if !dur.is_empty() {
        words.push(format!("for={dur}"));
    }
    words.push(format!("resume={resume}"));
    Some(Plan::Post {
        path: if all { PAUSE_ALL } else { PAUSE },
        words,
        printer: Printer::Pause,
    })
}

fn log(rest: &[&str], pj: &str) -> Option<Plan> {
    let mut limit = "";
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--limit" => limit = value(rest, i)?,
            _ => {
                return Some(Plan::Usage(
                    concat!(
                        "Error: usage: ",
                        cli_name!(),
                        " mail watch log [--limit n] [--json]"
                    )
                    .into(),
                ))
            }
        }
        i += 2;
    }
    let mut query = pj.to_string();
    if !limit.is_empty() {
        // curl puts it in the URL as it is.
        if limit.bytes().any(|b| b <= b' ' || b == 0x7f || b == b'#') {
            return None;
        }
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str("limit=");
        query.push_str(limit);
    }
    Some(Plan::Get {
        path: LOG,
        query,
        printer: Printer::Log,
    })
}

/// `json.dumps(v)`: Python's default spacing, non-ASCII escaped.
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("ASCII JSON always prints")
}

/// `digest --every`'s Python: an ASCII text made only of `<digits><s|m|h|d>`
/// groups (any case), as seconds. `None` where its Python exits 1 (and the
/// shell prints the not-a-duration line).
pub fn duration(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut total = String::from("0");
    let mut i = 0;
    let mut groups = 0;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let digits = &lower[start..i];
        let unit = match bytes.get(i) {
            Some(b's') => 1,
            Some(b'm') => 60,
            Some(b'h') => 3600,
            Some(b'd') => 86400,
            _ => return None,
        };
        if digits.is_empty() {
            return None;
        }
        // int() of more than 4300 digits raises.
        if digits.trim_start_matches('0').len() > big::MAX_DIGITS {
            return None;
        }
        total = big::add(&total, &big::mul_small(digits, unit));
        i += 1;
        groups += 1;
    }
    if groups == 0 || total.len() > big::MAX_DIGITS {
        return None;
    }
    Some(total)
}

/// `_mw_body`'s Python on `words`: the JSON body it prints, and the
/// exception that stopped it (the body is then empty).
pub fn body(words: &[String]) -> (String, Option<Raise>) {
    let mut out: Vec<(Text, Value)> = Vec::new();
    for w in words {
        let Some((k, v)) = w.split_once('=') else {
            return (
                String::new(),
                Some(Raise::at(
                    BODY_SPLIT_LINE,
                    "ValueError: not enough values to unpack (expected 2, got 1)",
                )),
            );
        };
        let value = if k == "project" {
            s(v)
        } else {
            json::parse(v).unwrap_or_else(|_| s(v))
        };
        let key = Text::from(k);
        match out.iter_mut().find(|(have, _)| *have == key) {
            Some(slot) => slot.1 = value,
            None => out.push((key, value)),
        }
    }
    (dumps(&Value::Object(out)), None)
}

/// What `reply=$(cli_send_json ...)` holds: one `\n000` for each refused
/// connect tried again, then the answer and `\n<status>`; for a call that
/// failed, curl's `\n000` for each try and the shell's own `\n000`. NULs
/// are dropped, as `$(…)` drops them.
pub fn reply(result: &Result<Response, Error>, refused: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    match result {
        Ok(r) => {
            for _ in 0..refused {
                raw.extend_from_slice(b"\n000");
            }
            raw.extend_from_slice(&r.body);
            raw.extend_from_slice(format!("\n{:03}", r.status).as_bytes());
        }
        Err(_) => {
            for _ in 0..refused + 2 {
                raw.extend_from_slice(b"\n000");
            }
        }
    }
    raw.retain(|b| *b != 0);
    while raw.last() == Some(&b'\n') {
        raw.pop();
    }
    raw
}

/// `_mw_python` on `reply` with the printer's source.
pub fn reader(reply: &[u8], printer: Printer, json_flag: bool) -> Shown {
    let (body, status) = match reply.iter().rposition(|b| *b == b'\n') {
        Some(at) => (&reply[..at], &reply[at + 1..]),
        None => (reply, reply),
    };
    let raw = String::from_utf8_lossy(body);
    let status = String::from_utf8_lossy(status);
    let d = if raw.is_empty() {
        Value::Object(Vec::new())
    } else {
        match json::parse(&raw) {
            Ok(d) => d,
            Err(_) => {
                let cut: String = raw.chars().take(200).collect();
                return Shown::err(format!("Error: {cut}\n"), Exit::Error);
            }
        }
    };
    let ok_false =
        matches!(&d, Value::Object(p) if super::super::get(p, "ok") == Some(&Value::Bool(false)));
    if status.starts_with('2') && !ok_false {
        let mut o = String::new();
        return match print(&d, printer, json_flag, &mut o) {
            Ok(()) => Shown::out(o),
            Err(r) => raised(o, String::new(), &r),
        };
    }
    let err = match &d {
        Value::Object(p) => super::super::get(p, "error").unwrap_or(&d).clone(),
        _ => s(&raw),
    };
    let Value::Object(e) = &err else {
        return Shown::err(format!("Error: {}\n", py::str_of(&err)), Exit::Error);
    };
    let pick = |k: &str| super::super::get(e, k).filter(|v| py::truthy(v)).cloned();
    let code = pick("code").unwrap_or_else(|| s("error"));
    let said = pick("hint")
        .or_else(|| pick("message"))
        .unwrap_or_else(|| code.clone());
    let Value::Str(t) = &said else {
        return raised(
            String::new(),
            String::new(),
            &Raise::at(ERR_LINE, concat_error(&said)),
        );
    };
    let exit = if code == s("owner_only") || code == s("forbidden") || status == "403" {
        Exit::Refused
    } else if code == s("usage") {
        Exit::Usage
    } else {
        Exit::Error
    };
    Shown::err(format!("Error: {}\n", text(t)), exit)
}

/// The exception raised at `line` of the printer, inside `_mw_python`'s
/// `exec`.
fn at(line: u32) -> impl Fn(String) -> Raise {
    move |exception| Raise {
        frames: vec![(EXEC_LINE, "<module>"), (line, "<module>")],
        exception,
    }
}

/// The printer's lines for a 2xx answer `d`, into `o`.
fn print(d: &Value, printer: Printer, json_flag: bool, o: &mut String) -> Result<(), Raise> {
    match printer {
        Printer::Status => status_lines(d, json_flag, o),
        Printer::VipList => {
            let e = at(1);
            let vips = py_index(d, "vips").map_err(&e)?;
            let j = join("\n", &vips).map_err(&e)?;
            line(o, if j.is_empty() { "No VIPs." } else { &j });
            Ok(())
        }
        Printer::VipSet => {
            let e = at(1);
            let vips = py_index(d, "vips").map_err(&e)?;
            let j = join(", ", &vips).map_err(&e)?;
            line(
                o,
                &format!("VIPs: {}", if j.is_empty() { "none" } else { &j }),
            );
            Ok(())
        }
        Printer::Pause => {
            let e = at(1);
            match attr_get(d, "pausedUntil").map_err(&e)? {
                None | Some(Value::Null) => line(o, "Resumed."),
                Some(_) => {
                    let p = py_index(d, "pausedUntil").map_err(&e)?;
                    line(o, &concat("Paused until ", &p).map_err(&e)?);
                }
            }
            Ok(())
        }
        Printer::Log => log_lines(d, json_flag, o),
    }
}

/// `_mw_status_print`.
fn status_lines(d: &Value, json_flag: bool, o: &mut String) -> Result<(), Raise> {
    if json_flag {
        line(o, &indented(d));
        return Ok(());
    }
    let enabled = py_index(d, "enabled").map_err(at(4))?;
    line(
        o,
        &format!(
            "Mail notices: {}",
            if py::truthy(&enabled) { "on" } else { "off" }
        ),
    );
    let mut ibx = Vec::new();
    for i in iter(&py_index(d, "inboxes").map_err(at(5))?).map_err(at(5))? {
        if attr_get(&i, "wake")
            .map_err(at(5))?
            .is_some_and(|w| py::truthy(&w))
        {
            ibx.push(i);
        }
    }
    let mut others = Vec::new();
    for i in iter(&py_index(d, "inboxes").map_err(at(6))?).map_err(at(6))? {
        if !attr_get(&i, "wake")
            .map_err(at(6))?
            .is_some_and(|w| py::truthy(&w))
        {
            others.push(py_index(&i, "address").map_err(at(6))?);
        }
    }
    let mut chosen = Vec::new();
    for i in &ibx {
        let genexpr = |exception| Raise {
            frames: vec![(EXEC_LINE, "<module>"), (7, "<module>"), (7, "<genexpr>")],
            exception,
        };
        let a = py_index(i, "address").map_err(genexpr)?;
        let w = py_index(i, "wake").map_err(genexpr)?;
        chosen.push(format!("{} ({})", py::str_of(&a), py::str_of(&w)));
    }
    let chosen = chosen.join(", ");
    line(
        o,
        &format!(
            "Inboxes chosen: {}",
            if chosen.is_empty() { "none" } else { &chosen }
        ),
    );
    if !others.is_empty() {
        let j = join(", ", &Value::Array(others)).map_err(at(8))?;
        line(o, &format!("Can also choose: {j}"));
    }
    let vips = join(", ", &py_index(d, "vips").map_err(at(9))?).map_err(at(9))?;
    line(
        o,
        &format!("VIPs: {}", if vips.is_empty() { "none" } else { &vips }),
    );
    let dg = or_empty(attr_get(d, "digest").map_err(at(10))?);
    if py::truthy(&dg) {
        let days = attr_get(&dg, "days")
            .map_err(at(12))?
            .filter(py::truthy)
            .unwrap_or(Value::Array(Vec::new()));
        let win = if attr_get(&dg, "start")
            .map_err(at(13))?
            .is_some_and(|v| py::truthy(&v))
        {
            let start = attr_get(&dg, "start")
                .map_err(at(13))?
                .unwrap_or(Value::Null);
            let end = attr_get(&dg, "end").map_err(at(13))?.unwrap_or(Value::Null);
            let days = if py::truthy(&days) {
                format!(" {}", join(",", &days).map_err(at(13))?)
            } else {
                String::new()
            };
            format!("{}-{}{}", py::str_of(&start), py::str_of(&end), days)
        } else {
            "off".to_string()
        };
        let tz = if attr_get(&dg, "tz")
            .map_err(at(14))?
            .is_some_and(|v| py::truthy(&v))
        {
            format!(" ({})", py::str_of(&py_index(&dg, "tz").map_err(at(14))?))
        } else {
            String::new()
        };
        line(o, &format!("Check window: {win}{tz}"));
        let every = attr_get(&dg, "everySeconds")
            .map_err(at(15))?
            .filter(py::truthy)
            .unwrap_or(Value::Int("3600".into()));
        let minutes = big::floor_div(&py_int(&every).map_err(at(15))?, 60);
        let next = attr_get(&dg, "nextFireAt")
            .map_err(at(15))?
            .filter(py::truthy)
            .unwrap_or_else(|| s("-"));
        line(
            o,
            &format!("Digest every {minutes} min; next: {}", py::str_of(&next)),
        );
    }
    let word = |key: &str, yes: &'static str, no: &'static str| {
        py_index(d, key)
            .map(|v| if py::truthy(&v) { yes } else { no })
            .map_err(at(16))
    };
    let outside = word("vipOutsideWindow", "deliver", "hold")?;
    let session = word("vipSessionLine", "on", "off")?;
    let snippets = word("snippets", "on", "off")?;
    line(
        o,
        &format!(
            "VIP mail outside the window: {outside}; session line: {session}; snippets: {snippets}"
        ),
    );
    if attr_get(d, "pausedUntil")
        .map_err(at(17))?
        .is_some_and(|v| py::truthy(&v))
    {
        let p = py_index(d, "pausedUntil").map_err(at(17))?;
        line(o, &concat("Paused until ", &p).map_err(at(17))?);
    }
    for i in &ibx {
        if attr_get(i, "needsRelink")
            .map_err(at(19))?
            .is_some_and(|v| py::truthy(&v))
        {
            let a = py_index(i, "address").map_err(at(19))?;
            line(
                o,
                &format!(
                    "Stopped: {a} refused its sign-in. A person signs in again (Settings → Email Link → Sign in again, or {cli} mail link relink {a}) and it starts again.",
                    a = py::str_of(&a),
                    cli = k2_client::brand::CLI_NAME
                ),
            );
        } else if attr_get(i, "lastError")
            .map_err(at(20))?
            .is_some_and(|v| py::truthy(&v))
        {
            let a = py_index(i, "address").map_err(at(20))?;
            let e = py_index(i, "lastError").map_err(at(20))?;
            line(
                o,
                &format!(
                    "Last poll problem for {}: {}",
                    py::str_of(&a),
                    py::str_of(&e)
                ),
            );
        }
    }
    let c = or_empty(attr_get(d, "last24h").map_err(at(21))?);
    let count = |key: &str| {
        attr_get(&c, key)
            .map(|v| py::str_of(&v.unwrap_or(Value::Int("0".into()))))
            .map_err(at(22))
    };
    let vip = count("vipDelivered")?;
    let digested = count("digested")?;
    let failed = count("failedAuth")?;
    line(
        o,
        &format!(
            "Last 24h: {vip} VIP delivered, {digested} digested, {failed} failed authentication"
        ),
    );
    Ok(())
}

/// `log`'s printer.
fn log_lines(d: &Value, json_flag: bool, o: &mut String) -> Result<(), Raise> {
    if json_flag {
        line(o, &indented(d));
        return Ok(());
    }
    let rows = py_index(d, "log").map_err(at(4))?;
    let rows = if py::truthy(&rows) {
        rows
    } else {
        Value::Array(Vec::new())
    };
    for r in iter(&rows).map_err(at(4))? {
        let e = at(5);
        let field = |key: &str| py_index(&r, key).map_err(&e);
        let received = field("receivedAt")?;
        let class = field("class")?;
        let state = field("state")?;
        let from = field("from")?;
        let inbox = field("inbox")?;
        let reason = if attr_get(&r, "reason")
            .map_err(&e)?
            .is_some_and(|v| py::truthy(&v))
        {
            let why = py_index(&r, "reason").map_err(&e)?;
            format!("{})", concat("  (", &why).map_err(&e)?)
        } else {
            String::new()
        };
        line(
            o,
            &format!(
                "{}  {} {} {} -> {}{}",
                py::str_of(&received),
                super::super::py_left(&class, 8),
                super::super::py_left(&state, 10),
                py::str_of(&from),
                py::str_of(&inbox),
                reason
            ),
        );
    }
    if !py::truthy(&py_index(d, "log").map_err(at(6))?) {
        line(o, "Nothing yet.");
    }
    Ok(())
}

fn line(o: &mut String, text: &str) {
    o.push_str(text);
    o.push('\n');
}

/// `json.dumps(d, indent=2)`.
fn indented(d: &Value) -> String {
    json::to_string(
        d,
        &Options {
            indent: Some(2),
            ..Options::default()
        },
    )
    .expect("ASCII JSON always prints")
}

/// `x or {}`.
fn or_empty(v: Option<Value>) -> Value {
    v.filter(py::truthy).unwrap_or(Value::Object(Vec::new()))
}
