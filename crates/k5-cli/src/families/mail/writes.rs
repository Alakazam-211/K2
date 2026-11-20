//! The mail writes, each sent once: `send`, `reply`, `forward-message`,
//! `draft`, `move`, `flag`, `archive` and `delete` (a message to Trash, or
//! an address retired), as their `cmd_mail_*` functions read the words
//! (`cli/src/mail-watch.sh`, `mail-manage.sh`) and `_mail_py`'s verbs send
//! and print (`cli/src/mail.sh`). `outbox cancel` and `folder create` and
//! `rename` are with the reads ([`super::reads`]), under their verbs.
//!
//! A POST goes out once: the adapter tries again only where nothing was
//! sent (a refused connect) or no handler ran (the 503 "migrating").
//!
//! The binary hands these to the shell: a `--body-file` that is not UTF-8,
//! holds a NUL (bash drops it and warns, naming its script) or can't be
//! read (`cat`'s own line); a `--from-name` made only of white space
//! where some of it is not ASCII (bash's `[[:space:]]` follows the
//! locale); a number past bash's 64-bit integers.
//!
//! Where this differs from the shell: the body rides `M_BODY` in the
//! environment of the shell's `python3`, so a body over 128 KiB (Linux's
//! limit for one string there) stops the shell with `Argument list too
//! long` (exit 126) before anything is sent. The binary sends it: the
//! named difference `mail-body-large`.

use super::core::{self, run_env, usage, At, Env, Plan, Py, Secs, Within, MODULE, R};
use super::py::{self as p, attr_get};
use super::reads::check_timeout;
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::py;
use crate::transport::urllib::Method;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &[
    "send",
    "reply",
    "forward-message",
    "draft",
    "move",
    "flag",
    "archive",
    "delete",
];

/// The lines of `_mail_py` where these verbs call or raise. `tests`
/// pins each to its statement.
pub mod line {
    pub const OUTCOME_TIMED_OUT: u32 = 385;
    pub const OUTCOME_DIE: u32 = 387;
    pub const OUTCOME_QUEUED: u32 = 393;
    pub const OUTCOME_QUEUED_PRINT: u32 = 396;
    pub const OUTCOME_HAS: u32 = 399;
    pub const OUTCOME_DECIDED: u32 = 403;
    pub const OUTCOME_NOTE: u32 = 404;
    pub const OUTCOME_NOTE_PRINT: u32 = 405;
    pub const OUTCOME_TRACK: u32 = 406;
    pub const OUTCOME_SUBMITTED: u32 = 410;
    pub const SEND_CALL: u32 = 743;
    pub const SEND_OUTCOME: u32 = 744;
    pub const REPLY_CALL: u32 = 769;
    pub const REPLY_OUTCOME: u32 = 770;
    pub const FORWARD_CALL: u32 = 792;
    pub const FORWARD_OUTCOME: u32 = 793;
    pub const ADDRESS_DELETE_CALL: u32 = 849;
    pub const ADDRESS_DELETE_PRINT: u32 = 854;
    pub const DRAFT_CALL: u32 = 1639;
    pub const DRAFT_PRINT: u32 = 1643;
    pub const MOVE_CALL: u32 = 1957;
    pub const MOVE_PRINT: u32 = 1962;
    pub const FLAG_CALL: u32 = 1970;
    pub const FLAG_PRINT: u32 = 1979;
    pub const ARCHIVE_CALL: u32 = 1982;
    pub const ARCHIVE_PRINT: u32 = 1986;
    pub const DELETE_CALL: u32 = 1990;
    pub const DELETE_PRINT: u32 = 1994;
}

/// A value flag: its word, the `M_*` key it sets, whether a repeat adds a
/// line (`to="${to}${to:+$'\n'}$2"`) or replaces it, and the hint when
/// its value is missing.
struct Takes<'h> {
    word: &'static str,
    key: &'static str,
    join: bool,
    hint: &'h str,
}

const fn set<'h>(word: &'static str, key: &'static str, hint: &'h str) -> Takes<'h> {
    Takes {
        word,
        key,
        join: false,
        hint,
    }
}

const fn add<'h>(word: &'static str, key: &'static str, hint: &'h str) -> Takes<'h> {
    Takes {
        word,
        key,
        join: true,
        hint,
    }
}

/// What a `while [ $# -gt 0 ]` loop read: the `M_*` values, and each
/// value flag that came (even with an empty value).
#[derive(Default)]
struct Read {
    env: Env,
    seen: Vec<&'static str>,
}

impl Read {
    fn get(&self, key: &str) -> &str {
        self.env.get(key)
    }

    fn has(&self, key: &str) -> bool {
        self.seen.contains(&key)
    }
}

/// One `while` loop: value flags, switches (`(word, key, value)`), the
/// unknown-flag hint for a word starting with `-`, and what a positional
/// word does.
fn walk<'a>(
    words: &[&'a str],
    takes: &[Takes],
    switches: &[(&str, &'static str, &'static str)],
    unknown: &dyn Fn(&str) -> String,
    positional: &mut dyn FnMut(&mut Read, &'a str) -> Result<(), Plan>,
) -> Result<Read, Plan> {
    let mut r = Read::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if let Some(t) = takes.iter().find(|t| t.word == w) {
            let Some(v) = words.get(i + 1) else {
                return Err(usage(t.hint));
            };
            let now = r.get(t.key).to_string();
            let next = if t.join && !now.is_empty() {
                format!("{now}\n{v}")
            } else {
                (*v).to_string()
            };
            r.env.set(t.key, next);
            r.seen.push(t.key);
            i += 2;
            continue;
        }
        if let Some((_, key, value)) = switches.iter().find(|(word, _, _)| *word == w) {
            r.env.set(key, *value);
            r.seen.push(key);
        } else if w.starts_with('-') {
            return Err(usage(unknown(w)));
        } else {
            positional(&mut r, w)?;
        }
        i += 1;
    }
    Ok(r)
}

/// `body="$(cat "$body_file")"` after `[ ! -f "$body_file" ]`: the text
/// with its trailing newlines gone, or the usage line. `None`: the
/// shell's (a NUL, not UTF-8, or `cat` can't read it).
fn body_file(path: &str) -> Option<Result<String, Plan>> {
    let is_file = std::fs::metadata(path).is_ok_and(|m| m.is_file());
    if !is_file {
        return Some(Err(usage(format!("--body-file: no such file '{path}'"))));
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(Ok(text.trim_end_matches('\n').to_string()))
}

/// `[ -z "${from_name//[[:space:]]/}" ]`: `Some(true)` when only white
/// space; `None` where the locale decides (white space past ASCII).
fn blank_name(name: &str) -> Option<bool> {
    let rest: Vec<char> = name
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'))
        .collect();
    if rest.is_empty() {
        return Some(true);
    }
    if rest.iter().all(|c| c.is_whitespace()) {
        return None;
    }
    Some(false)
}

/// The body's checks, `--from-name`'s and `--timeout`'s, in the order
/// `cmd_mail_send` and `cmd_mail_reply` make them after their own.
fn body_name_timeout(r: &mut Read, need_body: &str) -> Result<(), Plan> {
    if !r.get("M_BODY").is_empty() && !r.get("M_BODY_FILE").is_empty() {
        return Err(usage("--body and --body-file are mutually exclusive"));
    }
    let file = r.get("M_BODY_FILE").to_string();
    if !file.is_empty() {
        match body_file(&file) {
            None => return Err(Plan::Shell),
            Some(Err(p)) => return Err(p),
            Some(Ok(text)) => r.env.set("M_BODY", text),
        }
    }
    if r.get("M_BODY").is_empty() {
        return Err(usage(need_body));
    }
    name_and_timeout(r)
}

fn name_and_timeout(r: &Read) -> Result<(), Plan> {
    if r.has("M_FROM_NAME") {
        match blank_name(r.get("M_FROM_NAME")) {
            None => return Err(Plan::Shell),
            Some(true) => {
                return Err(usage(
                    "--from-name needs a non-empty display name (omit it to use your workspace's agent name)",
                ))
            }
            Some(false) => {}
        }
    }
    let t = r.get("M_TIMEOUT");
    if !t.is_empty() {
        if !r.env.on("M_WAIT") {
            return Err(usage("--timeout only makes sense with --wait"));
        }
        match check_timeout(t) {
            None => return Err(Plan::Shell),
            Some(Err(p)) => return Err(p),
            Some(Ok(())) => {}
        }
    }
    Ok(())
}

/// The words after `mail <verb>`, as the verb's `cmd_mail_*` reads them.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "send" => send(words),
        "reply" => reply(words),
        "forward-message" => forward(words),
        "draft" => draft(words),
        "move" => move_(words),
        "flag" => flag(words),
        "archive" => archive(words),
        "delete" => delete(words),
        _ => Err(Plan::Shell),
    };
    r.unwrap_or_else(|p| p)
}

/// The settings `_mail_py` reads, from what the loop read.
fn run(verb: &str, r: Read, keys: &[&'static str]) -> Plan {
    let mut e = run_env(verb, r.env.on("M_JSON"));
    for k in keys {
        e.set(k, r.get(k));
    }
    Plan::Run(e)
}

fn json_switch() -> (&'static str, &'static str, &'static str) {
    ("--json", "M_JSON", "true")
}

/// `cmd_mail_send`.
fn send(words: &[&str]) -> Result<Plan, Plan> {
    let from_hint = format!("--from requires one of your addresses (see '{CLI_NAME} mail addresses')");
    let takes = [
        add("--to", "M_TO", "--to requires a recipient address"),
        set("--subject", "M_SUBJECT", "--subject requires a value"),
        set("--body", "M_BODY", "--body requires the message text"),
        set("--body-file", "M_BODY_FILE", "--body-file requires a file path"),
        add("--cc", "M_CC", "--cc requires an address"),
        add("--bcc", "M_BCC", "--bcc requires an address"),
        set("--from", "M_FROM", &from_hint),
        set(
            "--from-name",
            "M_FROM_NAME",
            "--from-name requires a display name (e.g. --from-name \"Laura\")",
        ),
        add("--reply-to", "M_REPLY_TO", "--reply-to requires an address"),
        add("--attach", "M_ATTACH", "--attach requires a file path"),
        set("--at", "M_SEND_AT", "--at requires an RFC3339 timestamp"),
        set("--in", "M_SEND_IN", "--in requires a duration (e.g. 30m, 2h, 1d)"),
        set(
            "--timeout",
            "M_TIMEOUT",
            "--timeout requires a value in seconds (1-900)",
        ),
    ];
    let switches = [("--wait", "M_WAIT", "true"), json_switch()];
    let unknown = |f: &str| {
        format!("unknown flag '{f}' — usage: {CLI_NAME} mail send <to> --subject <s> (--body <text> | --body-file <f>) [--cc <addr>] [--bcc <addr>] [--from <owned-addr>] [--from-name <name>] [--attach <path>] [--wait [--timeout <s>]] [--json]")
    };
    let mut r = walk(words, &takes, &switches, &unknown, &mut |r: &mut Read, w: &str| {
        let to = r.get("M_TO").to_string();
        r.env.set(
            "M_TO",
            if to.is_empty() {
                w.to_string()
            } else {
                format!("{to}\n{w}")
            },
        );
        Ok(())
    })?;
    if r.get("M_TO").is_empty() {
        return Err(usage(
            "send requires a recipient — positional <to> or --to <addr>",
        ));
    }
    if r.get("M_SUBJECT").is_empty() {
        return Err(usage("send requires --subject"));
    }
    body_name_timeout(&mut r, "send requires --body <text> or --body-file <f>")?;
    let (at, inn) = (r.get("M_SEND_AT"), r.get("M_SEND_IN"));
    if !at.is_empty() && !inn.is_empty() {
        return Err(usage(
            "pass only one of --at <when> or --in <duration>, not both",
        ));
    }
    if r.env.on("M_WAIT") && (!at.is_empty() || !inn.is_empty()) {
        return Err(usage("--wait cannot combine with --at/--in"));
    }
    Ok(run(
        "send",
        r,
        &[
            "M_TO",
            "M_SUBJECT",
            "M_BODY",
            "M_CC",
            "M_BCC",
            "M_FROM",
            "M_ATTACH",
            "M_FROM_NAME",
            "M_REPLY_TO",
            "M_WAIT",
            "M_TIMEOUT",
            "M_SEND_AT",
            "M_SEND_IN",
        ],
    ))
}

/// `cmd_mail_forward_message`.
fn forward(words: &[&str]) -> Result<Plan, Plan> {
    let use_line = format!("usage: {CLI_NAME} mail forward-message <message-id> --to <addr> [--cc <addr>] [--bcc <addr>] [--note <text>] [--as-eml] [--from <owned-addr>] [--from-name <name>] [--reply-to <addr> | --reply-to-original] [--wait [--timeout <s>]] [--json]");
    let from_hint = format!("--from requires one of your addresses (see '{CLI_NAME} mail addresses')");
    let takes = [
        add("--to", "M_TO", "--to requires a recipient address"),
        add("--cc", "M_CC", "--cc requires an address"),
        add("--bcc", "M_BCC", "--bcc requires an address"),
        set("--note", "M_NOTE", "--note requires your text"),
        set("--from", "M_FROM", &from_hint),
        set("--from-name", "M_FROM_NAME", "--from-name requires a display name"),
        add("--reply-to", "M_REPLY_TO", "--reply-to requires an address"),
        set(
            "--timeout",
            "M_TIMEOUT",
            "--timeout requires a value in seconds (1-900)",
        ),
    ];
    let switches = [
        ("--as-eml", "M_AS_EML", "true"),
        ("--reply-to-original", "M_REPLY_TO_ORIGINAL", "true"),
        ("--wait", "M_WAIT", "true"),
        json_switch(),
    ];
    let unknown = |f: &str| format!("unknown flag '{f}' — {use_line}");
    let r = walk(words, &takes, &switches, &unknown, &mut |r: &mut Read, w: &str| {
        if r.get("M_ID").is_empty() {
            r.env.set("M_ID", w);
            Ok(())
        } else {
            Err(usage(format!(
                "unexpected extra argument '{w}' — one <message-id>; recipients go in --to"
            )))
        }
    })?;
    if r.get("M_ID").is_empty() {
        return Err(usage(format!(
            "forward-message requires a <message-id> (from '{CLI_NAME} mail messages') — {use_line}"
        )));
    }
    if r.get("M_TO").is_empty() {
        return Err(usage("forward-message requires --to <addr>"));
    }
    if !r.get("M_REPLY_TO").is_empty() && r.env.on("M_REPLY_TO_ORIGINAL") {
        return Err(usage(
            "pass only one of --reply-to <addr> or --reply-to-original",
        ));
    }
    name_and_timeout(&r)?;
    Ok(run(
        "forward_message",
        r,
        &[
            "M_ID",
            "M_TO",
            "M_CC",
            "M_BCC",
            "M_NOTE",
            "M_AS_EML",
            "M_FROM",
            "M_FROM_NAME",
            "M_REPLY_TO",
            "M_REPLY_TO_ORIGINAL",
            "M_WAIT",
            "M_TIMEOUT",
        ],
    ))
}

/// `cmd_mail_reply`.
fn reply(words: &[&str]) -> Result<Plan, Plan> {
    let takes = [
        add("--cc", "M_CC", "--cc requires an address"),
        add("--bcc", "M_BCC", "--bcc requires an address"),
        set("--body", "M_BODY", "--body requires the reply text"),
        set("--body-file", "M_BODY_FILE", "--body-file requires a file path"),
        set(
            "--from-name",
            "M_FROM_NAME",
            "--from-name requires a display name (e.g. --from-name \"Laura\")",
        ),
        add("--attach", "M_ATTACH", "--attach requires a file path"),
        set(
            "--timeout",
            "M_TIMEOUT",
            "--timeout requires a value in seconds (1-900)",
        ),
    ];
    let switches = [
        ("--reply-all", "M_REPLY_ALL", "true"),
        ("--wait", "M_WAIT", "true"),
        json_switch(),
    ];
    let unknown = |f: &str| {
        if f == "--reply-to" {
            format!("{CLI_NAME} mail reply goes back to the original sender and takes no --reply-to — send a new message with {CLI_NAME} mail send if answers should go elsewhere")
        } else {
            format!("unknown flag '{f}' — usage: {CLI_NAME} mail reply <message-id> (--body <text> | --body-file <f>) [--reply-all] [--cc <addr>] [--bcc <addr>] [--from-name <name>] [--attach <path>] [--wait [--timeout <s>]] [--json]")
        }
    };
    let mut r = walk(words, &takes, &switches, &unknown, &mut |r: &mut Read, w: &str| {
        if r.get("M_ID").is_empty() {
            r.env.set("M_ID", w);
            Ok(())
        } else {
            Err(usage(format!(
                "unexpected extra argument '{w}' — one <message-id> only"
            )))
        }
    })?;
    if r.get("M_ID").is_empty() {
        return Err(usage(format!(
            "reply requires a <message-id> (from '{CLI_NAME} mail messages')"
        )));
    }
    body_name_timeout(&mut r, "reply requires --body <text> or --body-file <f>")?;
    Ok(run(
        "reply",
        r,
        &[
            "M_ID",
            "M_BODY",
            "M_ATTACH",
            "M_FROM_NAME",
            "M_WAIT",
            "M_TIMEOUT",
            "M_CC",
            "M_BCC",
            "M_REPLY_ALL",
        ],
    ))
}

/// `cmd_mail_draft`.
fn draft(words: &[&str]) -> Result<Plan, Plan> {
    let reply_to_hint =
        format!("--reply-to requires a message id (from '{CLI_NAME} mail messages')");
    let from_hint = format!("--from requires a linked address (see '{CLI_NAME} mail inboxes')");
    // `--reply-to <id>` is a value flag that checks the id already given,
    // so the loop is written out here.
    let takes = [
        set("--body", "M_BODY", "--body requires the draft text"),
        set("--body-file", "M_BODY_FILE", "--body-file requires a file path"),
        add("--to", "M_TO", "--to requires a recipient address"),
        set("--subject", "M_SUBJECT", "--subject requires a value"),
        add("--cc", "M_CC", "--cc requires an address"),
        add("--bcc", "M_BCC", "--bcc requires an address"),
        set("--from", "M_FROM", &from_hint),
        add("--attach", "M_ATTACH", "--attach requires a file path"),
    ];
    let unknown = |f: &str| format!("unknown flag '{f}' — usage: {CLI_NAME} mail draft <message-id> (--body <text> | --body-file <f>) [--reply-all] [--cc <addr>] [--bcc <addr>] [--attach <path>]  OR  {CLI_NAME} mail draft --to <addr> --subject <s> (--body <text> | --body-file <f>) [--cc <addr>] [--bcc <addr>] [--attach <path>] [--from <linked>]");
    let mut r = Read::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w == "--reply-to" {
            let Some(v) = words.get(i + 1) else {
                return Err(usage(reply_to_hint));
            };
            let id = r.get("M_ID");
            if !id.is_empty() && id != *v {
                return Err(usage(
                    "one message to reply to — pass it once (as <message-id> or --reply-to)",
                ));
            }
            r.env.set("M_ID", *v);
            i += 2;
            continue;
        }
        if let Some(t) = takes.iter().find(|t| t.word == w) {
            let Some(v) = words.get(i + 1) else {
                return Err(usage(t.hint));
            };
            let now = r.get(t.key).to_string();
            r.env.set(
                t.key,
                if t.join && !now.is_empty() {
                    format!("{now}\n{v}")
                } else {
                    (*v).to_string()
                },
            );
            i += 2;
            continue;
        }
        match w {
            "--reply-all" => r.env.set("M_REPLY_ALL", "true"),
            "--json" => r.env.set("M_JSON", "true"),
            f if f.starts_with('-') => return Err(usage(unknown(f))),
            pos => {
                let id = r.get("M_ID");
                if id.is_empty() {
                    r.env.set("M_ID", pos);
                } else if id != pos {
                    return Err(usage(format!(
                        "unexpected extra argument '{pos}' — one <message-id> only"
                    )));
                }
            }
        }
        i += 1;
    }
    let id = r.get("M_ID").to_string();
    if !id.is_empty() && (!r.get("M_TO").is_empty() || !r.get("M_SUBJECT").is_empty()) {
        return Err(usage("pass either a <message-id> (reply draft) or --to/--subject (compose draft), not both"));
    }
    if id.is_empty() && r.env.on("M_REPLY_ALL") {
        return Err(usage(format!(
            "--reply-all needs the message you're answering: {CLI_NAME} mail draft <message-id> --reply-all"
        )));
    }
    if !id.is_empty() && !r.get("M_FROM").is_empty() {
        return Err(usage(
            "--from is for compose drafts (picks the linked inbox)",
        ));
    }
    if id.is_empty() {
        if r.get("M_TO").is_empty() {
            return Err(usage("compose draft requires --to <addr> --subject <s> (--body|--body-file); or pass a <message-id> to draft a reply"));
        }
        if r.get("M_SUBJECT").is_empty() {
            return Err(usage("compose draft requires --subject"));
        }
    }
    if !r.get("M_BODY").is_empty() && !r.get("M_BODY_FILE").is_empty() {
        return Err(usage("--body and --body-file are mutually exclusive"));
    }
    let file = r.get("M_BODY_FILE").to_string();
    if !file.is_empty() {
        match body_file(&file) {
            None => return Err(Plan::Shell),
            Some(Err(p)) => return Err(p),
            Some(Ok(text)) => r.env.set("M_BODY", text),
        }
    }
    if r.get("M_BODY").is_empty() {
        return Err(usage("draft requires --body <text> or --body-file <f>"));
    }
    Ok(run(
        "draft",
        r,
        &[
            "M_ID",
            "M_BODY",
            "M_TO",
            "M_SUBJECT",
            "M_CC",
            "M_BCC",
            "M_FROM",
            "M_ATTACH",
            "M_REPLY_ALL",
        ],
    ))
}

/// The positional words and `--json` of a manage verb (`move`, `archive`,
/// `delete`, `flag`'s id), with its own switches.
fn manage<'a>(
    words: &[&'a str],
    switches: &[(&str, &'static str, &'static str)],
    unknown: &dyn Fn(&str) -> String,
    slots: usize,
    extra: &dyn Fn(&str) -> String,
) -> Result<(Read, Vec<&'a str>), Plan> {
    let mut pos: Vec<&'a str> = Vec::new();
    let r = walk(words, &[], switches, unknown, &mut |_: &mut Read, w: &'a str| {
        if let Some(slot) = pos.iter_mut().find(|s| s.is_empty()) {
            *slot = w;
        } else if pos.len() < slots {
            pos.push(w);
        } else {
            return Err(usage(extra(w)));
        }
        Ok(())
    })?;
    Ok((r, pos))
}

/// `cmd_mail_move`.
fn move_(words: &[&str]) -> Result<Plan, Plan> {
    let (r, pos) = manage(
        words,
        &[json_switch()],
        &|f| format!("unknown flag '{f}' — usage: {CLI_NAME} mail move <message-id> <folder> [--json]"),
        2,
        &|w| format!("unexpected extra argument '{w}'"),
    )?;
    let at = |n: usize| pos.get(n).copied().unwrap_or("");
    if at(0).is_empty() {
        return Err(usage("move requires <message-id> <folder>"));
    }
    if at(1).is_empty() {
        return Err(usage(format!(
            "move requires a destination <folder> (see '{CLI_NAME} mail folder list <address>')"
        )));
    }
    Ok(Plan::Run(
        run_env("move", r.env.on("M_JSON"))
            .with("M_ID", at(0))
            .with("M_FOLDER", at(1)),
    ))
}

/// `cmd_mail_flag`.
fn flag(words: &[&str]) -> Result<Plan, Plan> {
    let (r, pos) = manage(
        words,
        &[
            ("--read", "M_READ", "true"),
            ("--unread", "M_READ", "false"),
            ("--star", "M_FLAGGED", "true"),
            ("--flag", "M_FLAGGED", "true"),
            ("--unstar", "M_FLAGGED", "false"),
            ("--unflag", "M_FLAGGED", "false"),
            json_switch(),
        ],
        &|f| format!("unknown flag '{f}' — usage: {CLI_NAME} mail flag <message-id> [--read|--unread] [--star|--unstar] [--json]"),
        1,
        &|w| format!("unexpected extra argument '{w}' — one <message-id> only"),
    )?;
    let id = pos.first().copied().unwrap_or("");
    if id.is_empty() {
        return Err(usage("flag requires a <message-id>"));
    }
    if r.get("M_READ").is_empty() && r.get("M_FLAGGED").is_empty() {
        return Err(usage(
            "flag needs at least one of --read/--unread or --star/--unstar",
        ));
    }
    Ok(Plan::Run(
        run_env("flag", r.env.on("M_JSON"))
            .with("M_ID", id)
            .with("M_READ", r.get("M_READ"))
            .with("M_FLAGGED", r.get("M_FLAGGED")),
    ))
}

/// `cmd_mail_archive`.
fn archive(words: &[&str]) -> Result<Plan, Plan> {
    let (r, pos) = manage(
        words,
        &[json_switch()],
        &|f| format!("unknown flag '{f}' — usage: {CLI_NAME} mail archive <message-id> [--json]"),
        1,
        &|w| format!("unexpected extra argument '{w}' — one <message-id> only"),
    )?;
    let id = pos.first().copied().unwrap_or("");
    if id.is_empty() {
        return Err(usage("archive requires a <message-id>"));
    }
    Ok(Plan::Run(
        run_env("archive", r.env.on("M_JSON")).with("M_ID", id),
    ))
}

/// `cmd_mail_delete`: a message id (`m_…`) to Trash, or an address
/// retired.
fn delete(words: &[&str]) -> Result<Plan, Plan> {
    let (r, pos) = manage(
        words,
        &[json_switch()],
        &|f| format!("unknown flag '{f}' — usage: {CLI_NAME} mail delete <message-id|address> [--json]"),
        1,
        &|w| format!("unexpected extra argument '{w}' — one <message-id|address> only"),
    )?;
    let arg = pos.first().copied().unwrap_or("");
    let json = r.env.on("M_JSON");
    if arg.is_empty() {
        return Err(usage("delete requires a <message-id> (moves the message to Trash, never erases it) or an <address> (retires the hosted address)"));
    }
    if arg.contains('@') {
        return Ok(Plan::Run(
            run_env("address_delete", json).with("M_ADDRESS", arg),
        ));
    }
    if arg.starts_with("m_") {
        return Ok(Plan::Run(run_env("delete", json).with("M_ID", arg)));
    }
    Err(usage(format!(
        "'{arg}' is neither a message id ('m_…', from '{CLI_NAME} mail messages') nor an address ('name@domain') — nothing deleted"
    )))
}

/// `_mail_py`'s branch for `verb`: `None` when it is not one of these.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "send" => send_py(py_),
        "reply" => reply_py(py_),
        "forward_message" => forward_py(py_),
        "draft" => draft_py(py_),
        "move" => move_py(py_),
        "flag" => flag_py(py_),
        "archive" => archive_py(py_),
        "delete" => delete_py(py_),
        "address_delete" => address_delete_py(py_),
        _ => return None,
    })
}

fn g(d: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(d, key).at(l, MODULE)
}

fn gs(d: &Value, key: &str, default: &str, l: u32) -> R<String> {
    Ok(py::str_of(&p::get_or(d, key, p::sv(default)).at(l, MODULE)?))
}

/// A body: `{"project": ws_path, …}` built in the Python's order.
struct Body(Vec<(&'static str, Value)>);

impl Body {
    fn new(py_: &Py) -> Body {
        Body(vec![("project", p::sv(&py_.ws))])
    }

    fn put(&mut self, k: &'static str, v: Value) {
        self.0.push((k, v));
    }

    fn s(&mut self, k: &'static str, v: &str) {
        self.put(k, p::sv(v));
    }

    /// `if env(k): body[key] = env(k)`.
    fn if_set(&mut self, py_: &Py, env: &str, k: &'static str) {
        if !py_.env(env).is_empty() {
            self.s(k, py_.env(env));
        }
    }

    /// `vals = env_list(k); if vals: body[key] = vals`.
    fn list(&mut self, py_: &Py, env: &str, k: &'static str) {
        let v = py_.env.list(env);
        if !v.is_empty() {
            self.put(k, Value::Array(v.iter().map(|x| p::sv(x)).collect()));
        }
    }

    fn value(self) -> Value {
        core::obj(self.0)
    }
}

/// `send_outcome(d, waiting, timeout_secs)`, from module line `from`.
fn send_outcome(py_: &mut Py, d: &Value, waiting: bool, t: u64, from: u32) -> R<()> {
    const F: &str = "send_outcome";
    let g = |k: &str, l: u32| attr_get(d, k).at(l, F).within(from, MODULE);
    let cli = CLI_NAME;
    if waiting && p::truthy(g("timedOut", line::OUTCOME_TIMED_OUT)?.as_ref()) {
        let hint = match g("hint", line::OUTCOME_DIE)?.filter(py::truthy) {
            Some(h) => h,
            None => {
                let id = p::get_or(d, "id", p::sv(""))
                    .at(line::OUTCOME_DIE, F)
                    .within(from, MODULE)?;
                p::sv(&format!(
                    "not decided within {t}s — the message is still queued; check '{cli} mail outbox {}'",
                    py::str_of(&id)
                ))
            }
        };
        return Err(py_.die(
            &core::obj(vec![("code", p::sv("timeout")), ("hint", hint)]),
            Some(Exit::Usage),
            None,
        ));
    }
    if py_.json {
        py_.print_json(d);
        return Ok(());
    }
    let id = |l: u32| -> R<String> {
        Ok(py::str_of(
            &p::get_or(d, "id", p::sv("")).at(l, F).within(from, MODULE)?,
        ))
    };
    if p::truthy(g("queued", line::OUTCOME_QUEUED)?.as_ref()) {
        match g("hint", line::OUTCOME_QUEUED_PRINT)?.filter(py::truthy) {
            Some(h) => py_.print_v(&h),
            None => {
                let i = id(line::OUTCOME_QUEUED_PRINT)?;
                py_.print(&format!(
                    "queued for approval ({i}) — track with '{cli} mail outbox {i}'"
                ));
            }
        }
        return Ok(());
    }
    let has = p::contains(d, "timedOut")
        .at(line::OUTCOME_HAS, F)
        .within(from, MODULE)?;
    if has {
        let i = id(line::OUTCOME_DECIDED)?;
        let st = match g("statusNote", line::OUTCOME_DECIDED)?.filter(py::truthy) {
            Some(v) => v,
            None => p::get_or(d, "status", p::sv(""))
                .at(line::OUTCOME_DECIDED, F)
                .within(from, MODULE)?,
        };
        py_.print(&format!("{i} {}", py::str_of(&st)));
        if p::truthy(g("note", line::OUTCOME_NOTE)?.as_ref()) {
            let n = p::py_index(d, "note")
                .at(line::OUTCOME_NOTE_PRINT, F)
                .within(from, MODULE)?;
            py_.print(&format!("note from your human: {}", py::str_of(&n)));
        }
        let i = id(line::OUTCOME_TRACK)?;
        py_.print(&format!("track with: {cli} mail outbox {i}"));
        return Ok(());
    }
    let i = id(line::OUTCOME_SUBMITTED)?;
    let h = match g("hint", line::OUTCOME_SUBMITTED)?.filter(py::truthy) {
        Some(v) => v,
        None => p::get_or(d, "status", p::sv(""))
            .at(line::OUTCOME_SUBMITTED, F)
            .within(from, MODULE)?,
    };
    py_.print(&format!("{i} {}", py::str_of(&h)));
    Ok(())
}

/// `wait_timeout_secs()`, and `--wait`'s `wait` and `timeout` keys.
fn waiting(py_: &Py, b: &mut Body) -> (bool, u64) {
    let waiting = py_.env.on("M_WAIT");
    let t = super::reads::wait_timeout_secs(py_);
    if waiting {
        b.put("wait", Value::Bool(true));
        if !py_.env("M_TIMEOUT").is_empty() {
            b.put("timeout", Value::Int(t.to_string()));
        }
    }
    (waiting, t)
}

/// `send`.
fn send_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.put(
        "to",
        Value::Array(py_.env.list("M_TO").iter().map(|x| p::sv(x)).collect()),
    );
    b.s("subject", py_.env("M_SUBJECT"));
    b.s("body", py_.env("M_BODY"));
    b.list(py_, "M_CC", "cc");
    b.list(py_, "M_BCC", "bcc");
    b.if_set(py_, "M_FROM", "from");
    b.if_set(py_, "M_FROM_NAME", "fromName");
    b.list(py_, "M_REPLY_TO", "replyTo");
    let attach = py_.env.lines("M_ATTACH");
    if !attach.is_empty() {
        b.put(
            "attachments",
            Value::Array(attach.iter().map(|x| p::sv(x)).collect()),
        );
    }
    b.if_set(py_, "M_SEND_AT", "sendAt");
    b.if_set(py_, "M_SEND_IN", "sendIn");
    let (waiting, t) = waiting(py_, &mut b);
    let timeout = if waiting {
        t + 30
    } else if attach.is_empty() {
        30
    } else {
        180
    };
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/send",
        &[],
        Some(&b.value()),
        Secs::Int(timeout),
        None,
        line::SEND_CALL,
    )?;
    send_outcome(py_, &d, waiting, t, line::SEND_OUTCOME)
}

/// `reply`.
fn reply_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("id", py_.env("M_ID"));
    b.s("body", py_.env("M_BODY"));
    b.if_set(py_, "M_FROM_NAME", "fromName");
    if py_.env.on("M_REPLY_ALL") {
        b.put("replyAll", Value::Bool(true));
    }
    b.list(py_, "M_CC", "cc");
    b.list(py_, "M_BCC", "bcc");
    let attach = py_.env.lines("M_ATTACH");
    if !attach.is_empty() {
        b.put(
            "attachments",
            Value::Array(attach.iter().map(|x| p::sv(x)).collect()),
        );
    }
    let (waiting, t) = waiting(py_, &mut b);
    let timeout = if waiting {
        t + 30
    } else if attach.is_empty() {
        30
    } else {
        180
    };
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/reply",
        &[],
        Some(&b.value()),
        Secs::Int(timeout),
        None,
        line::REPLY_CALL,
    )?;
    send_outcome(py_, &d, waiting, t, line::REPLY_OUTCOME)
}

/// `forward_message`.
fn forward_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("id", py_.env("M_ID"));
    b.put(
        "to",
        Value::Array(py_.env.list("M_TO").iter().map(|x| p::sv(x)).collect()),
    );
    b.list(py_, "M_CC", "cc");
    b.list(py_, "M_BCC", "bcc");
    b.list(py_, "M_REPLY_TO", "replyTo");
    b.if_set(py_, "M_NOTE", "note");
    b.if_set(py_, "M_FROM", "from");
    b.if_set(py_, "M_FROM_NAME", "fromName");
    if py_.env.on("M_AS_EML") {
        b.put("asEml", Value::Bool(true));
    }
    if py_.env.on("M_REPLY_TO_ORIGINAL") {
        b.put("replyToOriginal", Value::Bool(true));
    }
    let (waiting, t) = waiting(py_, &mut b);
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/forward-message",
        &[],
        Some(&b.value()),
        Secs::Int(if waiting { t + 180 } else { 180 }),
        None,
        line::FORWARD_CALL,
    )?;
    send_outcome(py_, &d, waiting, t, line::FORWARD_OUTCOME)
}

/// `draft`.
fn draft_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("body", py_.env("M_BODY"));
    b.if_set(py_, "M_ID", "id");
    b.list(py_, "M_TO", "to");
    b.if_set(py_, "M_SUBJECT", "subject");
    b.list(py_, "M_CC", "cc");
    b.list(py_, "M_BCC", "bcc");
    b.if_set(py_, "M_FROM", "from");
    if py_.env.on("M_REPLY_ALL") {
        b.put("replyAll", Value::Bool(true));
    }
    let attach = py_.env.lines("M_ATTACH");
    if !attach.is_empty() {
        b.put(
            "attachments",
            Value::Array(attach.iter().map(|x| p::sv(x)).collect()),
        );
    }
    let d = py_.post("/cli/mail/draft", &b.value(), line::DRAFT_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DRAFT_PRINT;
    match g(&d, "hint", l)?.filter(py::truthy) {
        Some(h) => py_.print_v(&h),
        None => {
            let folder = gs(&d, "folder", "?", l)?;
            let address = gs(&d, "address", "?", l)?;
            py_.print(&format!("draft saved to '{folder}' in {address}"));
        }
    }
    Ok(())
}

/// `move`.
fn move_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("id", py_.env("M_ID"));
    b.s("folder", py_.env("M_FOLDER"));
    let d = py_.post("/cli/mail/move", &b.value(), line::MOVE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::MOVE_PRINT;
    let id = gs(&d, "id", "", l)?;
    let folder = p::or(g(&d, "folder", l)?, p::sv(py_.env("M_FOLDER")));
    py_.print(&format!("moved {id} -> {}", py::str_of(&folder)));
    Ok(())
}

/// `flag`.
fn flag_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("id", py_.env("M_ID"));
    let read = (!py_.env("M_READ").is_empty()).then(|| py_.env.on("M_READ"));
    let flagged = (!py_.env("M_FLAGGED").is_empty()).then(|| py_.env.on("M_FLAGGED"));
    if let Some(v) = read {
        b.put("read", Value::Bool(v));
    }
    if let Some(v) = flagged {
        b.put("flagged", Value::Bool(v));
    }
    let d = py_.post("/cli/mail/flag", &b.value(), line::FLAG_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let mut parts = Vec::new();
    if let Some(v) = read {
        parts.push(if v { "read" } else { "unread" });
    }
    if let Some(v) = flagged {
        parts.push(if v { "flagged" } else { "unflagged" });
    }
    let id = gs(&d, "id", "", line::FLAG_PRINT)?;
    let what = if parts.is_empty() {
        "no change".to_string()
    } else {
        parts.join(", ")
    };
    py_.print(&format!("{id}: {what}"));
    Ok(())
}

/// `archive`.
fn archive_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("id", py_.env("M_ID"));
    let d = py_.post("/cli/mail/archive", &b.value(), line::ARCHIVE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::ARCHIVE_PRINT;
    let id = gs(&d, "id", "", l)?;
    let folder = p::or(g(&d, "folder", l)?, p::sv("Archive"));
    py_.print(&format!("archived {id} → {}", py::str_of(&folder)));
    Ok(())
}

/// `delete`: the message to Trash.
fn delete_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("id", py_.env("M_ID"));
    let d = py_.post("/cli/mail/delete", &b.value(), line::DELETE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DELETE_PRINT;
    let id = gs(&d, "id", "", l)?;
    let folder = p::or(g(&d, "folder", l)?, p::sv("Trash"));
    py_.print(&format!(
        "deleted {id} -> {} (moved to Trash, not erased)",
        py::str_of(&folder)
    ));
    Ok(())
}

/// `address_delete`: `mail delete <address>` retires it.
fn address_delete_py(py_: &mut Py) -> R<()> {
    let mut b = Body::new(py_);
    b.s("address", py_.env("M_ADDRESS"));
    let d = py_.post("/cli/mail/address/delete", &b.value(), line::ADDRESS_DELETE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = gs(&d, "address", "?", line::ADDRESS_DELETE_PRINT)?;
    py_.print(&format!("retired {a} — its cap slot is free immediately."));
    py_.print("(the mailbox data is kept for the retention window, then purged)");
    Ok(())
}
