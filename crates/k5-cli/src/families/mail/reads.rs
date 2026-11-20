//! The mail reads, the agent's hot path: `inboxes` (also `addresses` and
//! `list`), `domains`, `messages`, `read`, `attachments`, `wait`,
//! `drafts`, `outbox` (and `outbox cancel`), `capabilities` and `folder`
//! (`list`, `create`, `rename`), as their `cmd_mail_*` functions read the
//! words (`cli/src/mail-wrappers.sh`, `mail-inboxes.sh`, `mail-watch.sh`,
//! `mail-manage.sh`) and `_mail_py`'s verbs print the answers
//! (`cli/src/mail.sh`).
//!
//! The binary hands these to the shell: a number past bash's 64-bit
//! integers where `[ … -lt … ]` reads one (bash's own error lines, with
//! its script's path in them: the plan's D6).

use super::core::{
    self, fits_bash, is_uint, print, run_env, usage, At, End, Plan, Py, Secs, Within, MODULE, R,
};
use super::py::{self as p, attr_get, iter};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::py;
use crate::transport::urllib::Method;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &[
    "inboxes",
    "addresses",
    "list",
    "domains",
    "messages",
    "read",
    "attachments",
    "wait",
    "drafts",
    "outbox",
    "capabilities",
    "folder",
];

/// The lines of `_mail_py` where these verbs call or raise. `tests`
/// pins each to its statement.
pub mod line {
    pub const DOMAINS_CALL: u32 = 539;
    pub const DOMAINS_ROWS: u32 = 540;
    pub const DOMAINS_PENDING: u32 = 541;
    pub const DOMAINS_NAMES: u32 = 546;
    pub const DOMAINS_FOR: u32 = 552;
    pub const DOMAINS_ROW: u32 = 553;
    pub const MESSAGES_CALL: u32 = 581;
    pub const MESSAGES_MSGS: u32 = 582;
    pub const MESSAGES_NEXT: u32 = 583;
    pub const MESSAGES_ERRORS: u32 = 587;
    pub const MESSAGES_FOR: u32 = 613;
    pub const MESSAGES_ATT: u32 = 615;
    pub const MESSAGES_UNREAD: u32 = 617;
    pub const MESSAGES_ROW: u32 = 619;
    pub const MESSAGES_WINDOW: u32 = 628;
    pub const MESSAGES_NOTE: u32 = 632;
    pub const READ_CALL: u32 = 642;
    pub const READ_RAW: u32 = 647;
    pub const READ_FULL: u32 = 649;
    pub const ATT_POST: u32 = 666;
    pub const ATT_GET: u32 = 669;
    pub const ATT_WROTE: u32 = 673;
    pub const ATT_ATTS: u32 = 676;
    pub const ATT_NONE: u32 = 678;
    pub const ATT_FOR: u32 = 680;
    pub const ATT_ROW: u32 = 681;
    pub const ATT_FETCH: u32 = 684;
    pub const WAIT_CALL: u32 = 696;
    pub const WAIT_TIMED_OUT: u32 = 697;
    pub const WAIT_DIE: u32 = 699;
    pub const WAIT_FULL: u32 = 705;
    pub const CANCEL_CALL: u32 = 796;
    pub const CANCEL_PRINT: u32 = 801;
    pub const OUTBOX_CALL: u32 = 809;
    pub const OUTBOX_DETAIL: u32 = 813;
    pub const OUTBOX_ROWS: u32 = 815;
    pub const OUTBOX_FOR: u32 = 819;
    pub const OUTBOX_ROW: u32 = 820;
    pub const DRAFTS_CALL: u32 = 827;
    pub const DRAFTS_ROWS: u32 = 831;
    pub const DRAFTS_FOR: u32 = 834;
    pub const DRAFTS_ST: u32 = 835;
    pub const DRAFTS_LINE: u32 = 836;
    pub const DRAFTS_EDITED: u32 = 837;
    pub const DRAFTS_EDITED_BY: u32 = 838;
    pub const DRAFTS_STATE: u32 = 839;
    pub const DRAFTS_STATE_BY: u32 = 840;
    pub const DRAFTS_OUTBOUND: u32 = 841;
    pub const DRAFTS_OUTBOUND_ID: u32 = 842;
    pub const CAPS_REQUEST: u32 = 1789;
    pub const CAPS_ERR: u32 = 1795;
    pub const CAPS_HINT: u32 = 1796;
    pub const CAPS_AGAIN: u32 = 1798;
    pub const CAPS_IS_ERR: u32 = 1802;
    pub const CAPS_DIE_ERR: u32 = 1803;
    pub const CAPS_ROWS: u32 = 1807;
    pub const CAPS_FOR: u32 = 1810;
    pub const CAPS_C: u32 = 1811;
    pub const CAPS_MAIL: u32 = 1812;
    pub const CAPS_CAL: u32 = 1813;
    pub const CAPS_ON: u32 = 1814;
    pub const CAPS_LVL: u32 = 1815;
    pub const CAPS_FIRST: u32 = 1816;
    pub const CAPS_SECOND: u32 = 1818;
    pub const CAPS_ACCT: u32 = 1820;
    pub const CAPS_ACCT_LINE: u32 = 1822;
    pub const CAPS_MISSING: u32 = 1826;
    pub const CAPS_MISSING_LINE: u32 = 1827;
    pub const INBOXES_CALL: u32 = 1870;
    pub const INBOXES_ROWS: u32 = 1874;
    pub const INBOXES_OWNER: u32 = 1877;
    pub const INBOXES_PRINT: u32 = 1878;
    pub const FOLDERS_CALL: u32 = 1998;
    pub const FOLDERS_LIST: u32 = 2003;
    pub const FOLDERS_FOR: u32 = 2007;
    pub const FOLDER_CREATE_CALL: u32 = 2011;
    pub const FOLDER_CREATE_PRINT: u32 = 2016;
    pub const FOLDER_RENAME_CALL: u32 = 2019;
    pub const FOLDER_RENAME_PRINT: u32 = 2025;
}

/// The words after `mail <verb>`, as the verb's `cmd_mail_*` reads them.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    match verb {
        "inboxes" | "addresses" | "list" => inboxes(words),
        "domains" => domains(words),
        "messages" => messages(words),
        "read" => read(words),
        "attachments" => attachments(words),
        "wait" => wait(words),
        "drafts" => drafts(words),
        "outbox" => outbox(words),
        "capabilities" => capabilities(words),
        "folder" => folder(words),
        _ => Plan::Shell,
    }
}

/// A flag's value: `[ $# -ge 2 ] || { _mail_err "usage" <hint>; exit 2; }`.
fn value<'a>(words: &[&'a str], i: usize, hint: &str) -> Result<&'a str, Plan> {
    words.get(i + 1).copied().ok_or_else(|| usage(hint))
}

/// `cmd_mail_inboxes`.
fn inboxes(words: &[&str]) -> Plan {
    let mut json = false;
    for w in words {
        match *w {
            "--json" => json = true,
            other => {
                return usage(format!(
                    "unexpected argument '{other}' — usage: {CLI_NAME} mail inboxes [--json]"
                ))
            }
        }
    }
    Plan::Run(run_env("inboxes", json))
}

/// `cmd_mail_domains`.
fn domains(words: &[&str]) -> Plan {
    let mut json = false;
    for w in words {
        match *w {
            "--json" => json = true,
            other => {
                return usage(format!(
                    "unexpected argument '{other}' — usage: {CLI_NAME} mail domains [--json]"
                ))
            }
        }
    }
    Plan::Run(run_env("domains", json))
}

/// `cmd_mail_messages`.
fn messages(words: &[&str]) -> Plan {
    let (mut address, mut unread, mut limit, mut query, mut from) = ("", false, "", "", "");
    let (mut json, mut since, mut before, mut folder, mut junk, mut offset) =
        (false, "", "", "", false, "");
    let use_line = format!(
        "{CLI_NAME} mail messages [<address>] [--unread] [--limit N] [--offset N] [--since <date>] [--before <date>] [--folder <name>] [--junk] [--query <text>] [--from <substr>] [--json]"
    );
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let slot = match w {
            "--limit" => Some((&mut limit, "--limit requires a number (1-200)")),
            "--offset" => Some((
                &mut offset,
                "--offset requires a whole number (0-based page cursor)",
            )),
            "--since" => Some((
                &mut since,
                "--since requires a date (YYYY-MM-DD or a relative age like 7d/24h/30m)",
            )),
            "--before" => Some((
                &mut before,
                "--before requires a date (YYYY-MM-DD or a relative age like 7d/24h/30m)",
            )),
            "--folder" => Some((&mut folder, "--folder requires a mailbox name")),
            "--query" => Some((
                &mut query,
                "--query requires a value (matches subject AND body)",
            )),
            "--from" => Some((
                &mut from,
                "--from requires a value (matches the From header)",
            )),
            _ => None,
        };
        if let Some((slot, hint)) = slot {
            match value(words, i, hint) {
                Ok(v) => *slot = v,
                Err(p) => return p,
            }
            i += 2;
            continue;
        }
        match w {
            "--unread" => unread = true,
            "--junk" => junk = true,
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage(format!("unknown flag '{f}' — usage: {use_line}"))
            }
            pos => {
                if address.is_empty() {
                    address = pos;
                } else {
                    return usage(format!(
                        "unexpected extra argument '{pos}' — one optional <address> only"
                    ));
                }
            }
        }
        i += 1;
    }
    if !limit.is_empty() {
        if is_uint(limit) && !fits_bash(limit) {
            return Plan::Shell;
        }
        if !is_uint(limit) || !(1..=200).contains(&core::bash_num(limit)) {
            return usage("--limit must be a number from 1 to 200 (default 25)");
        }
    }
    if !offset.is_empty() && !is_uint(offset) {
        return usage("--offset must be a whole number (0 = first page)");
    }
    if junk && !folder.is_empty() {
        return usage("use either --junk or --folder <name>, not both");
    }
    let flag = |b: bool| if b { "true" } else { "false" };
    Plan::Run(
        run_env("messages", json)
            .with("M_ADDRESS", address)
            .with("M_UNREAD", flag(unread))
            .with("M_LIMIT", limit)
            .with("M_QUERY", query)
            .with("M_FROM", from)
            .with("M_OFFSET", offset)
            .with("M_SINCE", since)
            .with("M_BEFORE", before)
            .with("M_FOLDER", folder)
            .with("M_JUNK", flag(junk)),
    )
}

/// One `<id>` and these switches: `cmd_mail_read`'s loop.
fn one_id<'a>(
    words: &[&'a str],
    switches: &[&str],
    flag_hint: &str,
) -> Result<(&'a str, Vec<String>), Plan> {
    let mut id = "";
    let mut on = Vec::new();
    for w in words {
        if switches.contains(w) {
            on.push((*w).to_string());
        } else if w.starts_with('-') {
            return Err(usage(format!("unknown flag '{w}' — usage: {flag_hint}")));
        } else if id.is_empty() {
            id = w;
        } else {
            return Err(usage(format!(
                "unexpected extra argument '{w}' — one <message-id> only"
            )));
        }
    }
    Ok((id, on))
}

/// `cmd_mail_read`.
fn read(words: &[&str]) -> Plan {
    let hint = format!("{CLI_NAME} mail read <message-id> [--html] [--raw] [--json]");
    let (id, on) = match one_id(words, &["--html", "--raw", "--json"], &hint) {
        Ok(x) => x,
        Err(p) => return p,
    };
    let has = |f: &str| on.iter().any(|w| w == f);
    if id.is_empty() {
        return usage(format!(
            "read requires a <message-id> (from '{CLI_NAME} mail messages')"
        ));
    }
    if has("--html") && has("--raw") {
        return usage("--html and --raw are mutually exclusive (--raw is the full RFC 822 blob)");
    }
    let flag = |b: bool| if b { "true" } else { "false" };
    Plan::Run(
        run_env("read", has("--json"))
            .with("M_ID", id)
            .with("M_HTML", flag(has("--html")))
            .with("M_RAW", flag(has("--raw"))),
    )
}

/// `cmd_mail_attachments`.
fn attachments(words: &[&str]) -> Plan {
    let (mut id, mut get, mut out, mut json) = ("", "", "", false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--get" => match value(words, i, "--get requires the 1-based attachment number") {
                Ok(v) => {
                    get = v;
                    i += 1;
                }
                Err(p) => return p,
            },
            "--out" => match value(words, i, "--out requires a workspace-relative path") {
                Ok(v) => {
                    out = v;
                    i += 1;
                }
                Err(p) => return p,
            },
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage(format!(
                    "unknown flag '{f}' — usage: {CLI_NAME} mail attachments <message-id> [--get <n> --out <path>] [--json]"
                ))
            }
            pos => {
                if id.is_empty() {
                    id = pos;
                } else {
                    return usage(format!(
                        "unexpected extra argument '{pos}' — one <message-id> only"
                    ));
                }
            }
        }
        i += 1;
    }
    if id.is_empty() {
        return usage(format!(
            "attachments requires a <message-id> (from '{CLI_NAME} mail messages')"
        ));
    }
    if !get.is_empty() {
        if is_uint(get) && !fits_bash(get) {
            return Plan::Shell;
        }
        if !is_uint(get) || core::bash_num(get) < 1 {
            return usage("--get takes the 1-based attachment number from the list");
        }
        if out.is_empty() {
            return usage("--get needs --out <path> (workspace-relative; the daemon writes the bytes there, nothing more)");
        }
    } else if !out.is_empty() {
        return usage("--out only makes sense with --get <n>");
    }
    Plan::Run(
        run_env("attachments", json)
            .with("M_ID", id)
            .with("M_GET", get)
            .with("M_OUT", out),
    )
}

/// `_mail_check_timeout`: `None` when bash's `[` can't read the number
/// (the shell's).
pub fn check_timeout(t: &str) -> Option<Result<(), Plan>> {
    if is_uint(t) && !fits_bash(t) {
        return None;
    }
    Some(if !is_uint(t) || !(1..=900).contains(&core::bash_num(t)) {
        Err(usage(
            "--timeout must be 1-900 seconds (default 300); loop the call to wait longer",
        ))
    } else {
        Ok(())
    })
}

/// `cmd_mail_wait`.
fn wait(words: &[&str]) -> Plan {
    let (mut to, mut from, mut subject, mut timeout, mut json) = ("", "", "", "", false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let slot = match w {
            "--to" => Some((&mut to, "--to requires an owned address")),
            "--from" => Some((&mut from, "--from requires a substring")),
            "--subject" => Some((&mut subject, "--subject requires a substring")),
            "--timeout" => Some((
                &mut timeout,
                "--timeout requires a value in seconds (1-900)",
            )),
            _ => None,
        };
        if let Some((slot, hint)) = slot {
            match value(words, i, hint) {
                Ok(v) => *slot = v,
                Err(p) => return p,
            }
            i += 2;
            continue;
        }
        match w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage(format!(
                    "unknown flag '{f}' — usage: {CLI_NAME} mail wait [--to <addr>] [--from <substr>] [--subject <substr>] [--timeout <secs>] [--json]"
                ))
            }
            pos => {
                return usage(format!(
                    "unexpected argument '{pos}' — wait takes filters as flags only"
                ))
            }
        }
        i += 1;
    }
    if !timeout.is_empty() {
        match check_timeout(timeout) {
            None => return Plan::Shell,
            Some(Err(p)) => return p,
            Some(Ok(())) => {}
        }
    }
    Plan::Run(
        run_env("wait", json)
            .with("M_TO", to)
            .with("M_FROM", from)
            .with("M_SUBJECT", subject)
            .with("M_TIMEOUT", timeout),
    )
}

/// `cmd_mail_drafts`.
fn drafts(words: &[&str]) -> Plan {
    let (mut json, mut limit) = (false, "");
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => json = true,
            "--limit" => match value(words, i, "--limit needs a number") {
                Ok(v) => {
                    limit = v;
                    i += 1;
                }
                Err(p) => return p,
            },
            other => {
                return usage(format!(
                    "unexpected '{other}' — usage: {CLI_NAME} mail drafts [--limit <n>] [--json]"
                ))
            }
        }
        i += 1;
    }
    Plan::Run(run_env("drafts", json).with("M_LIMIT", limit))
}

/// `cmd_mail_outbox`.
fn outbox(words: &[&str]) -> Plan {
    if words.first() == Some(&"cancel") {
        let (mut id, mut json) = ("", false);
        for w in &words[1..] {
            match *w {
                "--json" => json = true,
                f if f.starts_with('-') => {
                    return usage(format!(
                        "usage: {CLI_NAME} mail outbox cancel <id> [--json]"
                    ))
                }
                pos => {
                    if id.is_empty() {
                        id = pos;
                    } else {
                        return usage("one <id> only");
                    }
                }
            }
        }
        if id.is_empty() {
            return usage("outbox cancel requires out_… id");
        }
        return Plan::Run(run_env("outbox_cancel", json).with("M_ID", id));
    }
    let (mut id, mut limit, mut json) = ("", "", false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--id" => match value(words, i, "--id requires an outbound id (out_…)") {
                Ok(v) => {
                    id = v;
                    i += 1;
                }
                Err(p) => return p,
            },
            "--limit" => match value(words, i, "--limit requires a number (1-100)") {
                Ok(v) => {
                    limit = v;
                    i += 1;
                }
                Err(p) => return p,
            },
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage(format!(
                    "unknown flag '{f}' — usage: {CLI_NAME} mail outbox [<id>] [--limit N] [--json]"
                ))
            }
            pos => {
                if id.is_empty() {
                    id = pos;
                } else {
                    return usage(format!(
                        "unexpected extra argument '{pos}' — one optional <id> only"
                    ));
                }
            }
        }
        i += 1;
    }
    if !limit.is_empty() {
        if is_uint(limit) && !fits_bash(limit) {
            return Plan::Shell;
        }
        if !is_uint(limit) || !(1..=100).contains(&core::bash_num(limit)) {
            return usage("--limit must be a number from 1 to 100 (default 20)");
        }
    }
    Plan::Run(
        run_env("outbox", json)
            .with("M_ID", id)
            .with("M_LIMIT", limit),
    )
}

/// `cmd_mail_capabilities`.
fn capabilities(words: &[&str]) -> Plan {
    let (mut address, mut json) = ("", false);
    for w in words {
        match *w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage(format!(
                    "unknown flag '{f}' — usage: {CLI_NAME} mail capabilities [<address>] [--json]"
                ))
            }
            pos => {
                if address.is_empty() {
                    address = pos;
                } else {
                    return usage(format!("unexpected extra argument '{pos}'"));
                }
            }
        }
    }
    Plan::Run(run_env("capabilities", json).with("M_ADDRESS", address))
}

/// `cmd_mail_folder`.
fn folder(words: &[&str]) -> Plan {
    let Some((sub, rest)) = words.split_first() else {
        return Plan::Help("mail/delete");
    };
    if !matches!(*sub, "list" | "create" | "rename") {
        return usage(format!(
            "unknown folder command '{sub}' — usage: {CLI_NAME} mail folder <list|create|rename> <address> …"
        ));
    }
    let mut pos: Vec<&str> = Vec::new();
    let mut json = false;
    for w in rest {
        match *w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage(format!(
                    "unknown flag '{f}' — see {CLI_NAME} mail folder --help"
                ))
            }
            w => {
                // `p1`, `p2`, `p3`: each takes the word while it is empty.
                if let Some(slot) = pos.iter_mut().find(|s| s.is_empty()) {
                    *slot = w;
                } else if pos.len() < 3 {
                    pos.push(w);
                } else {
                    return usage(format!("unexpected extra argument '{w}'"));
                }
            }
        }
    }
    let at = |n: usize| pos.get(n).copied().unwrap_or("");
    match *sub {
        "list" => {
            if at(0).is_empty() {
                return usage(format!(
                    "folder list requires <address> ('{CLI_NAME} mail inboxes')"
                ));
            }
            Plan::Run(run_env("folder_list", json).with("M_ADDRESS", at(0)))
        }
        "create" => {
            if at(0).is_empty() || at(1).is_empty() {
                return usage("folder create requires <address> <name>");
            }
            Plan::Run(
                run_env("folder_create", json)
                    .with("M_ADDRESS", at(0))
                    .with("M_NAME", at(1)),
            )
        }
        _ => {
            if at(0).is_empty() || at(1).is_empty() || at(2).is_empty() {
                return usage("folder rename requires <address> <from> <to>");
            }
            Plan::Run(
                run_env("folder_rename", json)
                    .with("M_ADDRESS", at(0))
                    .with("M_FOLDER_FROM", at(1))
                    .with("M_FOLDER_TO", at(2)),
            )
        }
    }
}

/// `_mail_py`'s branch for `verb`: `None` when it is not one of these.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "inboxes" => inboxes_py(py_),
        "domains" => domains_py(py_),
        "messages" => messages_py(py_),
        "read" => read_py(py_),
        "attachments" => attachments_py(py_),
        "wait" => wait_py(py_),
        "drafts" => drafts_py(py_),
        "outbox" => outbox_py(py_),
        "outbox_cancel" => cancel_py(py_),
        "capabilities" => capabilities_py(py_),
        "folder_list" => folder_list_py(py_),
        "folder_create" => folder_create_py(py_),
        "folder_rename" => folder_rename_py(py_),
        _ => return None,
    })
}

/// `d.get(key)` at module line `l`.
fn g(d: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(d, key).at(l, MODULE)
}

/// `d.get(key, default)` at module line `l`, as `%s` prints it.
fn gs(d: &Value, key: &str, default: &str, l: u32) -> R<String> {
    Ok(py::str_of(
        &p::get_or(d, key, p::sv(default)).at(l, MODULE)?,
    ))
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `inboxes`.
fn inboxes_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/inboxes", &py_.project(), line::INBOXES_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(g(&d, "inboxes", line::INBOXES_ROWS)?, empty_list());
    // `bool(rows) and all(r.get("yourLevel") is None for r in rows)`.
    let mut owner = py::truthy(&rows);
    if owner {
        for r in iter(&rows).at(line::INBOXES_OWNER, MODULE)? {
            let lvl = attr_get(&r, "yourLevel")
                .at(line::INBOXES_OWNER, "<genexpr>")
                .within(line::INBOXES_OWNER, MODULE)?;
            if !matches!(lvl, None | Some(Value::Null)) {
                owner = false;
                break;
            }
        }
    }
    print::inboxes(py_, &rows, !owner).within(line::INBOXES_PRINT, MODULE)
}

/// `domains` (the agent view).
fn domains_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/domain/list", &py_.project(), line::DOMAINS_CALL)?;
    let rows = p::or(g(&d, "domains", line::DOMAINS_ROWS)?, empty_list());
    let pending = p::or(g(&d, "pending", line::DOMAINS_PENDING)?, empty_list());
    let cli = CLI_NAME;
    if py_.json {
        py_.print_json(&d);
    } else if !py::truthy(&rows) {
        if py::truthy(&pending) {
            let l = line::DOMAINS_NAMES;
            let mut names = Vec::new();
            for x in iter(&pending).at(l, MODULE)? {
                names.push(match &x {
                    Value::Str(_) => py::str_of(&x),
                    other => {
                        let v = p::get_or(other, "domain", other.clone())
                            .at(l, "<genexpr>")
                            .within(l, MODULE)?;
                        py::str_of(&v)
                    }
                });
            }
            py_.print(&format!(
                "No mintable domains yet. Pending: {}",
                names.join(", ")
            ));
            py_.print(&format!("details: {cli} hostmail domain list"));
        } else {
            py_.print(&format!(
                "No domains yet — add one: {cli} hostmail domain add <domain>"
            ));
        }
    } else {
        for r in iter(&rows).at(line::DOMAINS_FOR, MODULE)? {
            let name = gs(&r, "domain", "?", line::DOMAINS_ROW)?;
            let def = p::truthy(g(&r, "isDefault", line::DOMAINS_ROW)?.as_ref());
            py_.print(&format!("{name}{}", if def { "   (default)" } else { "" }));
        }
        py_.print("");
        py_.print(&format!(
            "mint here: {cli} hostmail create <localpart>[@<domain>]"
        ));
        if py::truthy(&pending) {
            py_.print(&format!(
                "pending (not mintable): {cli} hostmail domain list"
            ));
        }
    }
    Ok(())
}

/// `messages`.
fn messages_py(py_: &mut Py) -> R<()> {
    let e = py_.env.clone();
    let mut params = py_.project();
    let mut add = |k: &str, v: &str| params.push((k.to_string(), v.to_string()));
    if !e.get("M_ADDRESS").is_empty() {
        add("address", e.get("M_ADDRESS"));
    }
    if e.on("M_UNREAD") {
        add("unread", "1");
    }
    for (k, key) in [
        ("M_LIMIT", "limit"),
        ("M_OFFSET", "offset"),
        ("M_SINCE", "since"),
        ("M_BEFORE", "before"),
    ] {
        if !e.get(k).is_empty() {
            add(key, e.get(k));
        }
    }
    if e.on("M_JUNK") {
        add("junk", "1");
    } else if !e.get("M_FOLDER").is_empty() {
        add("folder", e.get("M_FOLDER"));
    }
    for (k, key) in [("M_QUERY", "query"), ("M_FROM", "from")] {
        if !e.get(k).is_empty() {
            add(key, e.get(k));
        }
    }
    let d = py_.get("/cli/mail/messages", &params, line::MESSAGES_CALL)?;
    let msgs = p::or(g(&d, "messages", line::MESSAGES_MSGS)?, empty_list());
    let next_offset = g(&d, "nextOffset", line::MESSAGES_NEXT)?;
    let inbox_errors = p::or(g(&d, "inboxErrors", line::MESSAGES_ERRORS)?, empty_list());
    let cli = CLI_NAME;
    if py_.json {
        py_.print_json(&d);
    } else if !py::truthy(&msgs) {
        let filtered = [
            "M_QUERY",
            "M_FROM",
            "M_ADDRESS",
            "M_SINCE",
            "M_BEFORE",
            "M_FOLDER",
        ]
        .iter()
        .any(|k| !e.get(k).is_empty())
            || e.on("M_UNREAD")
            || e.on("M_JUNK");
        py_.print(&format!(
            "No messages{}.",
            if filtered {
                " matching the filters"
            } else {
                ""
            }
        ));
    } else {
        for m in iter(&msgs).at(line::MESSAGES_FOR, MODULE)? {
            let mut extras = String::new();
            if p::truthy(g(&m, "hasAttachment", line::MESSAGES_ATT)?.as_ref()) {
                extras.push_str(" · attachment");
            }
            if p::truthy(g(&m, "unread", line::MESSAGES_UNREAD)?.as_ref()) {
                extras.push_str(" · UNREAD");
            }
            let l = line::MESSAGES_ROW;
            let id = gs(&m, "id", "", l)?;
            let from = print::fmt_addr(g(&m, "from", l)?.as_ref()).within(l, MODULE)?;
            let subject = p::or(g(&m, "subject", l)?, p::sv("(no subject)"));
            let date = g(&m, "date", l)?;
            py_.print(&format!(
                "{id} · {from} · {} · {}{extras}",
                py::str_of(&subject),
                core::iso_ago(date.as_ref())
            ));
        }
        py_.print("");
        py_.print(&format!("read one: {cli} mail read <id>"));
        if let Some(next) = next_offset.filter(|v| *v != Value::Null) {
            let mut parts: Vec<String> = Vec::new();
            if !e.get("M_ADDRESS").is_empty() {
                parts.push(e.get("M_ADDRESS").to_string());
            }
            if e.on("M_JUNK") {
                parts.push("--junk".into());
            } else if !e.get("M_FOLDER").is_empty() {
                parts.push(format!("--folder {}", e.get("M_FOLDER")));
            }
            if e.on("M_UNREAD") {
                parts.push("--unread".into());
            }
            for (k, flag) in [
                ("M_SINCE", "--since"),
                ("M_BEFORE", "--before"),
                ("M_QUERY", "--query"),
                ("M_FROM", "--from"),
                ("M_LIMIT", "--limit"),
            ] {
                if !e.get(k).is_empty() {
                    parts.push(format!("{flag} {}", e.get(k)));
                }
            }
            parts.push(format!("--offset {}", py::str_of(&next)));
            py_.print(&format!(
                "more:     {cli} mail messages {}",
                parts.join(" ")
            ));
        }
    }
    if !py_.json {
        let l = line::MESSAGES_WINDOW;
        let sw = p::or(g(&d, "searchWindow", l)?, empty_dict());
        let hint = attr_get(&sw, "hint").at(l, MODULE)?;
        if let Some(h) = hint.filter(py::truthy) {
            py_.print_v(&h);
        }
    }
    if !py_.json && py::truthy(&inbox_errors) {
        let n = p::len(&inbox_errors).at(line::MESSAGES_NOTE, MODULE)?;
        py_.print(&format!(
            "note:     {n} inbox(es) had errors and were skipped — read one directly ({cli} mail messages <address>) to see why."
        ));
    }
    Ok(())
}

/// `read`.
fn read_py(py_: &mut Py) -> R<()> {
    let mut params = py_.project();
    params.push(("id".into(), py_.env("M_ID").to_string()));
    let raw = py_.env.on("M_RAW");
    if py_.env.on("M_HTML") {
        params.push(("html".into(), "1".into()));
    }
    if raw {
        params.push(("raw".into(), "1".into()));
    }
    let d = py_.get("/cli/mail/read", &params, line::READ_CALL)?;
    if py_.json {
        py_.print_json(&d);
    } else if raw {
        let b = p::or(g(&d, "rawBase64", line::READ_RAW)?, p::sv(""));
        let bytes = b64decode(&b).at(line::READ_RAW, MODULE)?;
        py_.stdout.extend_from_slice(&bytes);
    } else {
        let m = p::or(g(&d, "message", line::READ_FULL)?, empty_dict());
        print::full(py_, &m).within(line::READ_FULL, MODULE)?;
    }
    Ok(())
}

/// `base64.b64decode(v)` (not `validate`): characters outside the
/// alphabet are skipped, and the data ends at a pad once two or more of a
/// group of four came; or the exception's text.
pub fn b64decode(v: &Value) -> Result<Vec<u8>, String> {
    let t = match v {
        Value::Str(t) => py::str_of(&Value::Str(t.clone())),
        other => {
            return Err(format!(
                "TypeError: argument should be a bytes-like object or ASCII string, not '{}'",
                py::type_name(other)
            ))
        }
    };
    if !t.is_ascii() {
        return Err("ValueError: string argument should contain only ASCII characters".into());
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    // binascii.a2b_base64 with strict_mode off.
    let (mut out, mut quad, mut bits, mut pads) = (Vec::new(), 0u32, 0u32, 0u32);
    for c in t.bytes() {
        if c == b'=' {
            if quad >= 2 {
                pads += 1;
                if quad + pads >= 4 {
                    return Ok(out);
                }
            }
            continue;
        }
        let Some(x) = val(c) else {
            continue;
        };
        pads = 0;
        bits = (bits << 6) | x;
        quad = (quad + 1) % 4;
        match quad {
            2 => out.push((bits >> 4) as u8),
            3 => out.push((bits >> 2) as u8),
            0 => out.push(bits as u8),
            _ => {}
        }
        bits &= match quad {
            1 => 0x3f,
            2 => 0x0f,
            3 => 0x03,
            _ => 0,
        };
    }
    match quad {
        0 => Ok(out),
        1 => Err(format!(
            "binascii.Error: Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            out.len() / 3 * 4 + 1
        )),
        _ => Err("binascii.Error: Incorrect padding".into()),
    }
}

/// `attachments` (the list, or `--get` written to a file by the daemon).
fn attachments_py(py_: &mut Py) -> R<()> {
    let mut params = py_.project();
    params.push(("id".into(), py_.env("M_ID").to_string()));
    let get = py_.env("M_GET").to_string();
    let out = py_.env("M_OUT").to_string();
    let d = if !get.is_empty() {
        params.push(("get".into(), get.clone()));
        params.push(("out".into(), out.clone()));
        let hint = |_: &Value| {
            format!(
                "the daemon did not finish attachment #{get} within the wait and is still downloading it; it writes to '{out}' (a folder gets '<folder>/<attachment name>') when done. Check for the file before retrying; a retry while it runs answers 'busy'"
            )
        };
        py_.call_full(
            Method::Post,
            "/cli/mail/attachments",
            &params,
            None,
            Secs::Int(300),
            Some(&hint as &dyn Fn(&Value) -> String),
            line::ATT_POST,
        )?
    } else {
        py_.get("/cli/mail/attachments", &params, line::ATT_GET)?
    };
    let cli = CLI_NAME;
    if py_.json {
        py_.print_json(&d);
    } else if !get.is_empty() {
        let l = line::ATT_WROTE;
        let index = g(&d, "index", l)?.unwrap_or(Value::Null);
        let name = p::or(g(&d, "filename", l)?, p::sv("?"));
        let mime = p::or(g(&d, "mime", l)?, p::sv("?"));
        let bytes = g(&d, "bytes", l)?.unwrap_or(Value::Null);
        let wrote = g(&d, "wrote", l)?.unwrap_or(Value::Null);
        py_.print(&format!(
            "wrote attachment #{}: {} ({}, {} bytes) -> {}",
            py::str_of(&index),
            py::str_of(&name),
            py::str_of(&mime),
            py::str_of(&bytes),
            py::str_of(&wrote)
        ));
    } else {
        let atts = p::or(g(&d, "attachments", line::ATT_ATTS)?, empty_list());
        if !py::truthy(&atts) {
            let id = gs(&d, "id", "", line::ATT_NONE)?;
            py_.print(&format!("No attachments on {id}."));
        } else {
            for a in iter(&atts).at(line::ATT_FOR, MODULE)? {
                let row = print::att_line(&a).at(line::ATT_ROW, MODULE)?;
                py_.print(&row);
            }
            py_.print("");
            let id = gs(&d, "id", "", line::ATT_FETCH)?;
            py_.print(&format!(
                "fetch one: {cli} mail attachments {id} --get <n> --out <path>   (n is 1-based)"
            ));
        }
    }
    Ok(())
}

/// `wait_timeout_secs()`: `M_TIMEOUT` (checked 1-900 by the words), else
/// 300.
pub fn wait_timeout_secs(py_: &Py) -> u64 {
    py_.env("M_TIMEOUT").parse().unwrap_or(300)
}

/// `wait`.
fn wait_py(py_: &mut Py) -> R<()> {
    let mut params = py_.project();
    for (k, key) in [
        ("M_TO", "to"),
        ("M_FROM", "from"),
        ("M_SUBJECT", "subject"),
        ("M_TIMEOUT", "timeout"),
    ] {
        if !py_.env(k).is_empty() {
            params.push((key.into(), py_.env(k).to_string()));
        }
    }
    let t = wait_timeout_secs(py_);
    let d = py_.call_full(
        Method::Get,
        "/cli/mail/wait",
        &params,
        None,
        Secs::Int(t + 30),
        None,
        line::WAIT_CALL,
    )?;
    if p::truthy(g(&d, "timedOut", line::WAIT_TIMED_OUT)?.as_ref()) {
        return Err(py_.die_with(
            "timeout",
            &format!(
                "no matching message within {t}s — loop '{CLI_NAME} mail wait' to keep listening (the blessed pattern)"
            ),
            Some(Exit::Usage),
        ));
    }
    if py_.json {
        py_.print_json(&d);
    } else {
        let m = p::or(g(&d, "message", line::WAIT_FULL)?, empty_dict());
        print::full(py_, &m).within(line::WAIT_FULL, MODULE)?;
    }
    Ok(())
}

/// `drafts`.
fn drafts_py(py_: &mut Py) -> R<()> {
    let mut params = py_.project();
    if !py_.env("M_LIMIT").is_empty() {
        params.push(("limit".into(), py_.env("M_LIMIT").to_string()));
    }
    let d = py_.get("/cli/mail/drafts", &params, line::DRAFTS_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(g(&d, "drafts", line::DRAFTS_ROWS)?, empty_list());
    if !py::truthy(&rows) {
        py_.print(&format!(
            "No drafted replies yet — '{CLI_NAME} mail draft <message-id> --body <text>' makes one."
        ));
    }
    for r in iter(&rows).at(line::DRAFTS_FOR, MODULE)? {
        let st = p::or(g(&r, "status", line::DRAFTS_ST)?, p::sv("?"));
        let l = line::DRAFTS_LINE;
        let id = g(&r, "id", l)?.unwrap_or(Value::Null);
        let inbox = g(&r, "inbox", l)?.unwrap_or(Value::Null);
        let reply_of = g(&r, "replyOf", l)?.unwrap_or(Value::Null);
        let mut text = format!(
            "{}  {} {} answers {}",
            py::str_of(&id),
            p::ljust(&py::str_of(&st), 9),
            p::ljust(&py::str_of(&inbox), 34),
            py::str_of(&reply_of)
        );
        if p::truthy(g(&r, "editedBy", line::DRAFTS_EDITED)?.as_ref()) {
            let by = p::py_index(&r, "editedBy").at(line::DRAFTS_EDITED_BY, MODULE)?;
            text.push_str(&format!("  · edited by {}", py::str_of(&by)));
        }
        let done = ["sent", "held", "discarded"]
            .iter()
            .any(|s| p::eq_str(&st, s));
        if done && p::truthy(g(&r, "stateBy", line::DRAFTS_STATE)?.as_ref()) {
            let by = p::py_index(&r, "stateBy").at(line::DRAFTS_STATE_BY, MODULE)?;
            text.push_str(&format!("  · {} by {}", py::str_of(&st), py::str_of(&by)));
        }
        if p::truthy(g(&r, "outboundId", line::DRAFTS_OUTBOUND)?.as_ref()) {
            let o = p::py_index(&r, "outboundId").at(line::DRAFTS_OUTBOUND_ID, MODULE)?;
            text.push_str(&format!("  · outbox {}", py::str_of(&o)));
        }
        py_.print(&text);
    }
    Ok(())
}

/// `outbox` (a list, or one with `M_ID`).
fn outbox_py(py_: &mut Py) -> R<()> {
    let mut params = py_.project();
    for (k, key) in [("M_ID", "id"), ("M_LIMIT", "limit")] {
        if !py_.env(k).is_empty() {
            params.push((key.into(), py_.env(k).to_string()));
        }
    }
    let d = py_.get("/cli/mail/outbox", &params, line::OUTBOX_CALL)?;
    if py_.json {
        py_.print_json(&d);
    } else if !py_.env("M_ID").is_empty() {
        let o = p::or(g(&d, "outbound", line::OUTBOX_DETAIL)?, empty_dict());
        print::outbound_detail(py_, &o).within(line::OUTBOX_DETAIL, MODULE)?;
    } else {
        let rows = p::or(g(&d, "outbox", line::OUTBOX_ROWS)?, empty_list());
        if !py::truthy(&rows) {
            py_.print("Outbox empty — nothing queued or sent yet.");
        } else {
            for r in iter(&rows).at(line::OUTBOX_FOR, MODULE)? {
                print::outbound_row(py_, &r).within(line::OUTBOX_ROW, MODULE)?;
            }
        }
    }
    Ok(())
}

/// `outbox cancel <id>`.
fn cancel_py(py_: &mut Py) -> R<()> {
    let body = core::obj(vec![
        ("project", p::sv(&py_.ws)),
        ("id", p::sv(py_.env("M_ID"))),
    ]);
    let d = py_.post("/cli/mail/outbox/cancel", &body, line::CANCEL_CALL)?;
    if py_.json {
        py_.print_json(&d);
    } else {
        let l = line::CANCEL_PRINT;
        match g(&d, "hint", l)?.filter(py::truthy) {
            Some(h) => py_.print_v(&h),
            None => {
                let id = gs(&d, "id", "", l)?;
                py_.print(&format!("cancelled {id}"));
            }
        }
    }
    Ok(())
}

/// `request("GET", "/cli/mail/capabilities", params)` at module line
/// `l`; no answer is `die`'s `daemon_unreachable`.
fn caps_request(py_: &Py, params: &[(String, String)], l: u32) -> R<Value> {
    let d = py_
        .request(
            Method::Get,
            "/cli/mail/capabilities",
            params,
            None,
            Secs::Int(30),
        )
        .within(l, MODULE)?;
    match d {
        Some(d) => Ok(d),
        None => Err(py_.unreachable()),
    }
}

/// `isinstance(d.get("error"), dict)`, at line `l`.
fn error_dict(d: &Value, l: u32) -> R<Option<Value>> {
    Ok(g(d, "error", l)?.filter(|e| matches!(e, Value::Object(_))))
}

/// `capabilities`: the agent view in a workspace, the owner's outside one.
fn capabilities_py(py_: &mut Py) -> R<()> {
    let mut params = Vec::new();
    if !py_.ws.is_empty() {
        params.push(("project".to_string(), py_.ws.clone()));
    }
    if !py_.env("M_ADDRESS").is_empty() {
        params.push(("address".to_string(), py_.env("M_ADDRESS").to_string()));
    }
    let mut d = caps_request(py_, &params, line::CAPS_REQUEST)?;
    let err = error_dict(&d, line::CAPS_ERR)?;
    if let Some(e) = err.filter(py::truthy) {
        if params.iter().any(|(k, _)| k == "project") {
            let h = p::get_or(&e, "hint", p::sv("")).at(line::CAPS_HINT, MODULE)?;
            if py::str_of(&h).starts_with("no agent/workspace named") {
                params.retain(|(k, _)| k != "project");
                d = caps_request(py_, &params, line::CAPS_AGAIN)?;
            }
        }
    }
    if let Some(e) = error_dict(&d, line::CAPS_IS_ERR)? {
        return Err(match py_.die(&e, None, None) {
            End::Raise(mut t) => {
                t.frames.insert(0, (line::CAPS_DIE_ERR, MODULE));
                End::Raise(t)
            }
            other => other,
        });
    }
    let cli = CLI_NAME;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = if !py_.env("M_ADDRESS").is_empty() {
        Value::Array(vec![d.clone()])
    } else {
        p::or(g(&d, "inboxes", line::CAPS_ROWS)?, empty_list())
    };
    if !py::truthy(&rows) {
        py_.print(&format!(
            "no inboxes — '{cli} mail inboxes' lists what you can reach"
        ));
    }
    for r in iter(&rows).at(line::CAPS_FOR, MODULE)? {
        let c = p::or(g(&r, "capabilities", line::CAPS_C)?, empty_dict());
        let mail = p::or(g(&c, "mail", line::CAPS_MAIL)?, empty_dict());
        let cal = p::or(g(&c, "calendar", line::CAPS_CAL)?, empty_dict());
        let on = |m: &Value| -> R<String> {
            let mut ks = Vec::new();
            for k in [
                "read", "draft", "compose", "send", "write", "invite", "rsvp",
            ] {
                let v = attr_get(m, k)
                    .at(line::CAPS_ON, "<genexpr>")
                    .within(line::CAPS_ON, "<lambda>")?;
                if v == Some(Value::Bool(true)) {
                    ks.push(k);
                }
            }
            Ok(if ks.is_empty() {
                "-".into()
            } else {
                ks.join(",")
            })
        };
        let lvl = g(&r, "yourLevel", line::CAPS_LVL)?;
        let l = line::CAPS_FIRST;
        let address = gs(&r, "address", "?", l)?;
        let source = gs(&c, "source", "?", l)?;
        let level = match lvl.filter(py::truthy) {
            Some(v) => format!("  level {}", py::str_of(&v)),
            None => String::new(),
        };
        py_.print(&format!("{address}  [{source}]{level}"));
        let l = line::CAPS_SECOND;
        let m_on = on(&mail).within(l, MODULE)?;
        let c_on = on(&cal).within(l, MODULE)?;
        let fb = gs(&cal, "freebusy", "none", l)?;
        py_.print(&format!(
            "  mail: {m_on}   calendar: {c_on}   free/busy: {fb}"
        ));
        let acct = g(&c, "account", line::CAPS_ACCT)?;
        if let Some(acct) = acct.filter(py::truthy) {
            let l = line::CAPS_ACCT_LINE;
            let kind = p::or(g(&acct, "kind", l)?, p::sv("unknown"));
            let tenant = if p::truthy(g(&acct, "tenant", l)?.as_ref()) {
                let t = p::py_index(&acct, "tenant").at(l, MODULE)?;
                format!(" tenant {}", py::str_of(&t))
            } else {
                String::new()
            };
            let checked = p::truthy(g(&acct, "checked", l)?.as_ref());
            py_.print(&format!(
                "  account: {}{tenant}{}",
                py::str_of(&kind),
                if checked {
                    String::new()
                } else {
                    format!(" (not checked — '{cli} mail link upgrade' checks it)")
                }
            ));
        }
        let missing = p::or(g(&c, "missing", line::CAPS_MISSING)?, empty_list());
        for m in iter(&missing).at(line::CAPS_MISSING, MODULE)? {
            let l = line::CAPS_MISSING_LINE;
            let cap = g(&m, "cap", l)?.unwrap_or(Value::Null);
            let reason = g(&m, "reason", l)?.unwrap_or(Value::Null);
            let fix = g(&m, "fix", l)?.unwrap_or(Value::Null);
            py_.print(&format!(
                "  - {} off ({}): {}",
                py::str_of(&cap),
                py::str_of(&reason),
                py::str_of(&fix)
            ));
        }
    }
    Ok(())
}

/// `folder list <address>`.
fn folder_list_py(py_: &mut Py) -> R<()> {
    let mut params = py_.project();
    params.push(("address".into(), py_.env("M_ADDRESS").to_string()));
    let d = py_.get("/cli/mail/folder/list", &params, line::FOLDERS_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let folders = p::or(g(&d, "folders", line::FOLDERS_LIST)?, empty_list());
    if !py::truthy(&folders) {
        py_.print("No folders.");
    } else {
        for f in iter(&folders).at(line::FOLDERS_FOR, MODULE)? {
            py_.print_v(&f);
        }
    }
    Ok(())
}

/// `folder create <address> <name>`.
fn folder_create_py(py_: &mut Py) -> R<()> {
    let body = core::obj(vec![
        ("project", p::sv(&py_.ws)),
        ("address", p::sv(py_.env("M_ADDRESS"))),
        ("name", p::sv(py_.env("M_NAME"))),
    ]);
    let d = py_.post("/cli/mail/folder/create", &body, line::FOLDER_CREATE_CALL)?;
    if py_.json {
        py_.print_json(&d);
    } else {
        let l = line::FOLDER_CREATE_PRINT;
        let name = p::or(g(&d, "folder", l)?, p::sv(py_.env("M_NAME")));
        let address = gs(&d, "address", "", l)?;
        py_.print(&format!(
            "created folder '{}' in {address}",
            py::str_of(&name)
        ));
    }
    Ok(())
}

/// `folder rename <address> <from> <to>`.
fn folder_rename_py(py_: &mut Py) -> R<()> {
    let body = core::obj(vec![
        ("project", p::sv(&py_.ws)),
        ("address", p::sv(py_.env("M_ADDRESS"))),
        ("from", p::sv(py_.env("M_FOLDER_FROM"))),
        ("to", p::sv(py_.env("M_FOLDER_TO"))),
    ]);
    let d = py_.post("/cli/mail/folder/rename", &body, line::FOLDER_RENAME_CALL)?;
    if py_.json {
        py_.print_json(&d);
    } else {
        let l = line::FOLDER_RENAME_PRINT;
        let from = gs(&d, "from", "", l)?;
        let to = gs(&d, "to", "", l)?;
        let address = gs(&d, "address", "", l)?;
        py_.print(&format!("renamed '{from}' -> '{to}' in {address}"));
    }
    Ok(())
}
