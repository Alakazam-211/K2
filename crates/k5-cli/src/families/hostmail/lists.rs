//! Mailing lists, spam, the outbound queue, mailbox sharing, client
//! autoconfig and the Sieve audit: `hostmail list`, `spam`, `queue`,
//! `acl`, `autoconfig` and `audit-sieve`, as their `cmd_hostmail_*`
//! functions read the words (`cli/src/hostmail-lists-spam-queue.sh`,
//! `cli/src/hostmail-ooo-footer.sh`) and `_mail_py`'s verbs answer
//! (`cli/src/mail.sh`).

use super::pyx::{self, g, items, or_dict, or_s, s_of};
use super::words::{scan, wants_help, Spec};
use crate::families::mail::core::{
    dumps, obj, run_env, usage, At, End, Plan, Py, Secs, Tb, MODULE, R,
};
use crate::families::mail::py::{self as p, attr_get};
use crate::out::json::Value;
use crate::out::py;
use crate::transport::urllib::Method;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["list", "spam", "queue", "acl", "autoconfig", "audit-sieve"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const LIST_CREATE_CALL: u32 = 2898;
    pub const LIST_CREATE_JOIN: u32 = 2903;
    pub const LIST_SHOW_CALL: u32 = 2906;
    pub const LIST_SHOW_JOIN: u32 = 2908;
    pub const LIST_MEMBERS_CALL: u32 = 2918;
    pub const LIST_MEMBERS_JOIN: u32 = 2920;
    pub const LIST_DELETE_CALL: u32 = 2923;
    pub const TRAIN_CALL: u32 = 2927;
    pub const SENDER_CALL: u32 = 2933;
    pub const QUAR_LIST_CALL: u32 = 2937;
    pub const QUAR_LIST_FOR: u32 = 2945;
    pub const QUAR_LIST_ROW: u32 = 2946;
    pub const QUAR_ID_CALL: u32 = 2951;
    pub const QUEUE_LIST_CALL: u32 = 2955;
    pub const QUEUE_LIST_FOR: u32 = 2963;
    pub const QUEUE_LIST_ROW: u32 = 2964;
    pub const QUEUE_RETRY_CALL: u32 = 2967;
    pub const QUEUE_DROP_CALL: u32 = 2971;
    pub const ACL_GRANT_CALL: u32 = 2986;
    pub const ACL_REVOKE_CALL: u32 = 2991;
    pub const ACL_SHOW_CALL: u32 = 2996;
    pub const AUTO_SHOW_CALL: u32 = 3000;
    pub const AUTO_SHOW_FOR: u32 = 3008;
    pub const AUTO_SHOW_ROW: u32 = 3009;
    pub const AUTO_APPLY_CALL: u32 = 3012;
    pub const AUTO_APPLY_FOR: u32 = 3020;
    pub const AUTO_APPLY_ROW: u32 = 3021;
    pub const AUDIT_CALL: u32 = 3027;
    pub const AUDIT_ENOTIFY: u32 = 3035;
    pub const AUDIT_SCANNED: u32 = 3036;
    pub const AUDIT_ACCOUNTS: u32 = 3047;
    pub const AUDIT_WHO: u32 = 3048;
    pub const AUDIT_FLAGS: u32 = 3050;
    pub const AUDIT_REDIRECTS: u32 = 3062;
    pub const AUDIT_REDIRECT: u32 = 3063;
    pub const AUDIT_SCRIPTS: u32 = 3064;
    pub const AUDIT_TARGETS: u32 = 3065;
    pub const AUDIT_TARGETS_OF: u32 = 3066;
    pub const AUDIT_SCRIPT: u32 = 3067;
    pub const AUDIT_NONE: u32 = 3068;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "list" => list(words),
        "spam" => spam(words),
        "queue" => queue(words),
        "acl" => acl(words),
        "autoconfig" => autoconfig(words),
        "audit-sieve" => audit(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// One positional word in `slot`, flags `values`, the usage lines given.
fn one(
    words: &[&str],
    slot: &'static str,
    values: &[(&'static str, &'static str, &str)],
    unknown: &str,
) -> Result<super::words::Got, Plan> {
    scan(
        words,
        &Spec {
            slots: &[slot],
            values,
            switches: &[],
            unknown,
            extra: "unexpected extra argument '{w}'",
        },
    )
}

/// `--member <a>` repeated, newline-joined, with one positional `<addr>`.
fn members(words: &[&str], unknown: &str) -> Result<(String, String, bool), Plan> {
    let (mut address, mut members, mut json) = (String::new(), Vec::new(), false);
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => json = true,
            "--member" => {
                let v = words
                    .get(i + 1)
                    .ok_or_else(|| usage("--member requires an address"))?;
                members.push(v.to_string());
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(super::words::fill(unknown, f)));
            }
            w => {
                if address.is_empty() {
                    address = w.into();
                } else {
                    return Err(usage(format!("unexpected extra argument '{w}'")));
                }
            }
        }
        i += 1;
    }
    // `members="$members\n$2"` only once members is not empty: an empty
    // first value is dropped by the next one.
    let mut joined = String::new();
    for m in members {
        if joined.is_empty() {
            joined = m;
        } else {
            joined = format!("{joined}\n{m}");
        }
    }
    Ok((address, joined, json))
}

/// `cmd_hostmail_list`.
fn list(words: &[&str]) -> Result<Plan, Plan> {
    const HELP: &str = "hostmail/list";
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::HelpUsage(HELP));
    };
    match *sub {
        "create" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let (a, m, json) = members(
                rest,
                &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail list create <addr> --member <a> [--member <a>…] [--json]"),
            )?;
            if a.is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail list create <addr> --member <a>…"
                )));
            }
            Ok(Plan::Run(
                run_env("list_create", json)
                    .with("M_ADDRESS", a)
                    .with("M_MEMBERS", m),
            ))
        }
        "show" => list_show(rest),
        "members" => match rest.first() {
            None => Ok(Plan::Help(HELP)),
            Some(&("add" | "remove")) => {
                let op = rest[0];
                let rest = &rest[1..];
                if wants_help(rest) {
                    return Ok(Plan::Help(HELP));
                }
                let (a, m, json) = members(
                    rest,
                    &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail list members {op} <addr> --member <a>…"),
                )?;
                if a.is_empty() {
                    return Err(usage(format!(
                        "{CLI_NAME} hostmail list members {op} <addr> --member <a>…"
                    )));
                }
                if m.is_empty() {
                    return Err(usage("give --member <addr>"));
                }
                Ok(Plan::Run(
                    run_env("list_members", json)
                        .with("M_ADDRESS", a)
                        .with("M_LIST_OP", op)
                        .with("M_MEMBERS", m),
                ))
            }
            Some(&("--help" | "-h")) => Ok(Plan::Help(HELP)),
            Some(_) => list_show(rest),
        },
        "delete" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "address",
                &[],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail list delete <addr> [--json]"
                ),
            )?;
            if g.get("address").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail list delete <addr>")));
            }
            Ok(Plan::Run(
                run_env("list_delete", g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help(HELP)),
        other => Err(usage(format!(
            "unknown list command '{other}' — usage: {CLI_NAME} hostmail list <create|members|delete|show>"
        ))),
    }
}

/// `cmd_hostmail_list_show`.
fn list_show(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/list"));
    }
    let g = one(
        words,
        "address",
        &[],
        &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail list show <addr> [--json]"),
    )?;
    if g.get("address").is_empty() {
        return Err(usage(format!("{CLI_NAME} hostmail list show <addr>")));
    }
    Ok(Plan::Run(
        run_env("list_show", g.json).with("M_ADDRESS", g.get("address")),
    ))
}

/// `cmd_hostmail_spam`.
fn spam(words: &[&str]) -> Result<Plan, Plan> {
    const HELP: &str = "hostmail/spam";
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help(HELP));
    };
    match *sub {
        "ham" | "spam" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "id",
                &[("--address", "address", "--address requires a value")],
                &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail spam {sub} <message-id> --address <addr> [--json]"),
            )?;
            if g.get("id").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail spam {sub} <message-id> --address <addr>"
                )));
            }
            if g.get("address").is_empty() {
                return Err(usage("--address is required (ids are per-inbox)"));
            }
            Ok(Plan::Run(
                run_env("spam_train", g.json)
                    .with("M_ADDRESS", g.get("address"))
                    .with("M_ID", g.get("id"))
                    .with("M_CLASS", *sub),
            ))
        }
        "allow" | "block" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "sender",
                &[],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail spam {sub} <sender> [--json]"
                ),
            )?;
            if g.get("sender").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail spam {sub} <sender>")));
            }
            Ok(Plan::Run(
                run_env("spam_sender", g.json)
                    .with("M_SENDER", g.get("sender"))
                    .with("M_LIST_OP", *sub),
            ))
        }
        "quarantine" => quarantine(rest),
        "--help" | "-h" => Ok(Plan::Help(HELP)),
        other => Err(usage(format!(
            "unknown spam command '{other}' — usage: {CLI_NAME} hostmail spam <ham|spam|allow|block|quarantine>"
        ))),
    }
}

/// `cmd_hostmail_spam_quarantine`.
fn quarantine(words: &[&str]) -> Result<Plan, Plan> {
    const HELP: &str = "hostmail/spam";
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help(HELP));
    };
    match *sub {
        "list" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "address",
                &[("--address", "address", "--address requires a value")],
                &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail spam quarantine list --address <addr> [--json]"),
            )?;
            if g.get("address").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail spam quarantine list --address <addr>"
                )));
            }
            Ok(Plan::Run(
                run_env("spam_quarantine_list", g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "release" | "discard" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "id",
                &[("--address", "address", "--address requires a value")],
                &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail spam quarantine {sub} <id> --address <addr> [--json]"),
            )?;
            if g.get("id").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail spam quarantine {sub} <id> --address <addr>"
                )));
            }
            if g.get("address").is_empty() {
                return Err(usage("--address is required"));
            }
            Ok(Plan::Run(
                run_env("spam_quarantine_id", g.json)
                    .with("M_ADDRESS", g.get("address"))
                    .with("M_ID", g.get("id"))
                    .with("M_LIST_OP", *sub),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help(HELP)),
        other => Err(usage(format!(
            "unknown quarantine command '{other}' — usage: {CLI_NAME} hostmail spam quarantine <list|release|discard>"
        ))),
    }
}

/// `cmd_hostmail_queue`.
fn queue(words: &[&str]) -> Result<Plan, Plan> {
    const HELP: &str = "hostmail/queue";
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help(HELP));
    };
    match *sub {
        "list" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let bad = format!(
                "unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail queue list [--json]"
            );
            let g = scan(
                rest,
                &Spec {
                    slots: &[],
                    values: &[],
                    switches: &[],
                    unknown: &bad,
                    extra: &bad,
                },
            )?;
            Ok(Plan::Run(run_env("queue_list", g.json)))
        }
        "retry" | "drop" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "id",
                &[],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail queue {sub} <id> [--json]"
                ),
            )?;
            if g.get("id").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail queue {sub} <id>")));
            }
            Ok(Plan::Run(
                run_env(&format!("queue_{sub}"), g.json).with("M_ID", g.get("id")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help(HELP)),
        other => Err(usage(format!(
            "unknown queue command '{other}' — usage: {CLI_NAME} hostmail queue <list|retry|drop>"
        ))),
    }
}

/// `cmd_hostmail_acl`.
fn acl(words: &[&str]) -> Result<Plan, Plan> {
    const HELP: &str = "hostmail/acl";
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help(HELP));
    };
    if matches!(*sub, "grant" | "revoke" | "show") && wants_help(rest) {
        return Ok(Plan::Help(HELP));
    }
    match *sub {
        "grant" => {
            let g = scan(
                rest,
                &Spec {
                    slots: &["mailbox"],
                    values: &[
                        ("--user", "user", "--user requires a minted address"),
                        ("--rights", "rights", "--rights requires lrswipkxtena"),
                    ],
                    switches: &[
                        ("--read", "read"),
                        ("--write", "write"),
                        ("--post", "post"),
                        ("--admin", "admin"),
                    ],
                    unknown: &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail acl grant <mailbox> --user <addr> --rights lrswipkxten | --read|--write|--post|--admin"),
                    extra: "unexpected extra argument '{w}'",
                },
            )?;
            if g.get("mailbox").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail acl grant <mailbox> --user <addr> …"
                )));
            }
            if g.get("user").is_empty() {
                return Err(usage("--user is required"));
            }
            let tf = |k: &str| if g.on(k) { "true" } else { "false" };
            Ok(Plan::Run(
                run_env("acl_grant", g.json)
                    .with("M_MAILBOX", g.get("mailbox"))
                    .with("M_USER", g.get("user"))
                    .with("M_RIGHTS", g.get("rights"))
                    .with("M_ACL_READ", tf("read"))
                    .with("M_ACL_WRITE", tf("write"))
                    .with("M_ACL_POST", tf("post"))
                    .with("M_ACL_ADMIN", tf("admin")),
            ))
        }
        "revoke" => {
            let g = one(
                rest,
                "mailbox",
                &[("--user", "user", "--user requires a minted address")],
                &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail acl revoke <mailbox> --user <addr> [--json]"),
            )?;
            if g.get("mailbox").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail acl revoke <mailbox> --user <addr>"
                )));
            }
            if g.get("user").is_empty() {
                return Err(usage("--user is required"));
            }
            Ok(Plan::Run(
                run_env("acl_revoke", g.json)
                    .with("M_MAILBOX", g.get("mailbox"))
                    .with("M_USER", g.get("user")),
            ))
        }
        "show" => {
            let g = one(
                rest,
                "mailbox",
                &[],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail acl show <mailbox> [--json]"
                ),
            )?;
            if g.get("mailbox").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail acl show <mailbox>")));
            }
            Ok(Plan::Run(
                run_env("acl_show", g.json).with("M_MAILBOX", g.get("mailbox")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help(HELP)),
        other => Err(usage(format!(
            "unknown acl command '{other}' — usage: {CLI_NAME} hostmail acl <grant|revoke|show>"
        ))),
    }
}

/// `cmd_hostmail_autoconfig`.
fn autoconfig(words: &[&str]) -> Result<Plan, Plan> {
    const HELP: &str = "hostmail/autoconfig";
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help(HELP));
    };
    match *sub {
        "show" | "apply" => {
            if wants_help(rest) {
                return Ok(Plan::Help(HELP));
            }
            let g = one(
                rest,
                "domain",
                &[],
                &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail autoconfig {sub} <domain> [--json]"),
            )?;
            if g.get("domain").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail autoconfig {sub} <domain>"
                )));
            }
            Ok(Plan::Run(
                run_env(&format!("autoconfig_{sub}"), g.json).with("M_DOMAIN", g.get("domain")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help(HELP)),
        other => Err(usage(format!(
            "unknown autoconfig command '{other}' — usage: {CLI_NAME} hostmail autoconfig <show|apply>"
        ))),
    }
}

/// `cmd_hostmail_audit_sieve`.
fn audit(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/audit-sieve"));
    }
    let bad =
        format!("unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail audit-sieve [--json]");
    let g = scan(
        words,
        &Spec {
            slots: &[],
            values: &[],
            switches: &[],
            unknown: &bad,
            extra: &bad,
        },
    )?;
    Ok(Plan::Run(run_env("audit_sieve", g.json)))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "list_create" => list_change_py(py_, "/cli/mail/list", None, line::LIST_CREATE_CALL),
        "list_show" => list_show_py(py_),
        "list_members" => {
            let key = if py_.env("M_LIST_OP") == "add" {
                "add"
            } else {
                "remove"
            };
            list_change_py(
                py_,
                "/cli/mail/list/members",
                Some(key),
                line::LIST_MEMBERS_CALL,
            )
        }
        "list_delete" => list_delete_py(py_),
        "spam_train" => train_py(py_),
        "spam_sender" => sender_py(py_),
        "spam_quarantine_list" => quarantine_list_py(py_),
        "spam_quarantine_id" => quarantine_id_py(py_),
        "queue_list" => queue_list_py(py_),
        "queue_retry" => queue_retry_py(py_),
        "queue_drop" => queue_drop_py(py_),
        "acl_grant" => acl_grant_py(py_),
        "acl_revoke" => acl_revoke_py(py_),
        "acl_show" => acl_show_py(py_),
        "autoconfig_show" => auto_show_py(py_),
        "autoconfig_apply" => auto_apply_py(py_),
        "audit_sieve" => audit_py(py_),
        _ => return None,
    })
}

/// `d.get(key) or env(env_key)`.
fn or_env_bare(py_: &Py, d: &Value, key: &str, env_key: &str, l: u32) -> R<String> {
    Ok(match g(d, key, l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => py_.env(env_key).to_string(),
    })
}

/// `"list %s members=%s" % (d.get("address") or env("M_ADDRESS"), ",".join(d.get("members") or []))`.
fn list_line(py_: &Py, d: &Value, la: u32, lj: u32) -> R<String> {
    let a = or_env_bare(py_, d, "address", "M_ADDRESS", la)?;
    let m = p::or(g(d, "members", lj)?, Value::Array(vec![]));
    let j = pyx::join(",", &m, lj)?;
    Ok(format!("list {a} members={j}"))
}

/// `list_create` (`key` None) and `list_members` (`key` add or remove).
fn list_change_py(py_: &mut Py, route: &str, key: Option<&str>, l: u32) -> R<()> {
    let members: Vec<Value> = py_.env.list("M_MEMBERS").iter().map(|m| p::sv(m)).collect();
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        (key.unwrap_or("members"), Value::Array(members)),
    ]);
    let d = py_.post(route, &body, l)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let (la, lj) = if key.is_none() {
        (line::LIST_CREATE_JOIN - 1, line::LIST_CREATE_JOIN)
    } else {
        (line::LIST_MEMBERS_JOIN, line::LIST_MEMBERS_JOIN)
    };
    let t = list_line(py_, &d, la, lj)?;
    py_.print(&t);
    Ok(())
}

fn list_show_py(py_: &mut Py) -> R<()> {
    let params = vec![("address".to_string(), py_.env("M_ADDRESS").to_string())];
    let d = py_.get("/cli/mail/list", &params, line::LIST_SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let t = list_line(py_, &d, line::LIST_SHOW_JOIN, line::LIST_SHOW_JOIN)?;
    py_.print(&t);
    Ok(())
}

fn list_delete_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("address", pyx::env_v(py_, "M_ADDRESS"))]);
    let d = py_.post("/cli/mail/list/delete", &body, line::LIST_DELETE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = or_env_bare(py_, &d, "address", "M_ADDRESS", line::LIST_DELETE_CALL + 1)?;
    py_.print(&format!("deleted list {a}"));
    Ok(())
}

fn train_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        ("messageId", pyx::env_v(py_, "M_ID")),
        ("class", pyx::env_v(py_, "M_CLASS")),
    ]);
    let d = py_.post("/cli/mail/spam/train", &body, line::TRAIN_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::TRAIN_CALL + 2;
    let id = or_env_bare(py_, &d, "id", "M_ID", l)?;
    let class = or_env_bare(py_, &d, "class", "M_CLASS", l)?;
    py_.print(&format!("trained {id} as {class}"));
    Ok(())
}

fn sender_py(py_: &mut Py) -> R<()> {
    let route = if py_.env("M_LIST_OP") == "allow" {
        "/cli/mail/spam/allow"
    } else {
        "/cli/mail/spam/block"
    };
    let body = obj(vec![("sender", pyx::env_v(py_, "M_SENDER"))]);
    let d = py_.post(route, &body, line::SENDER_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::SENDER_CALL + 1;
    let action = or_env_bare(py_, &d, "action", "M_LIST_OP", l)?;
    let key = or_env_bare(py_, &d, "key", "M_SENDER", l)?;
    py_.print(&format!("{action} {key}"));
    Ok(())
}

fn quarantine_list_py(py_: &mut Py) -> R<()> {
    let params = vec![("address".to_string(), py_.env("M_ADDRESS").to_string())];
    let d = py_.get("/cli/mail/spam/quarantine", &params, line::QUAR_LIST_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(
        g(&d, "messages", line::QUAR_LIST_FOR - 4)?,
        Value::Array(vec![]),
    );
    if !py::truthy(&rows) {
        py_.print("No quarantined messages.");
        return Ok(());
    }
    for m in p::iter(&rows).at(line::QUAR_LIST_FOR, MODULE)? {
        let l = line::QUAR_LIST_ROW;
        let id = or_s(g(&m, "id", l)?, "?");
        let subject = or_s(g(&m, "subject", l)?, "");
        py_.print(&format!("{id}  {subject}"));
    }
    Ok(())
}

fn quarantine_id_py(py_: &mut Py) -> R<()> {
    let op = py_.env("M_LIST_OP").to_string();
    let route = if op == "release" {
        "/cli/mail/spam/quarantine/release"
    } else {
        "/cli/mail/spam/quarantine/discard"
    };
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        ("id", pyx::env_v(py_, "M_ID")),
    ]);
    let d = py_.post(route, &body, line::QUAR_ID_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::QUAR_ID_CALL + 1;
    let id = or_env_bare(py_, &d, "id", "M_ID", l)?;
    let folder = or_s(g(&d, "folder", l)?, "?");
    py_.print(&format!("{op} {id} -> {folder}"));
    Ok(())
}

fn queue_list_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/queue", &[], line::QUEUE_LIST_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(
        g(&d, "messages", line::QUEUE_LIST_FOR - 4)?,
        Value::Array(vec![]),
    );
    if !py::truthy(&rows) {
        py_.print("Outbound queue is empty.");
        return Ok(());
    }
    for m in p::iter(&rows).at(line::QUEUE_LIST_FOR, MODULE)? {
        let l = line::QUEUE_LIST_ROW;
        let id = or_s(g(&m, "id", l)?, "?");
        let next = or_s(g(&m, "nextRetry", l)?, "-");
        let from = or_s(g(&m, "returnPath", l)?, "-");
        py_.print(&format!("{id}  retry={next}  from={from}"));
    }
    Ok(())
}

fn queue_retry_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("id", pyx::env_v(py_, "M_ID"))]);
    let d = py_.post("/cli/mail/queue/retry", &body, line::QUEUE_RETRY_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::QUEUE_RETRY_CALL + 1;
    let id = or_env_bare(py_, &d, "id", "M_ID", l)?;
    let next = or_s(g(&d, "nextRetry", l)?, "?");
    py_.print(&format!("retry {id} next={next}"));
    Ok(())
}

fn queue_drop_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("id", pyx::env_v(py_, "M_ID"))]);
    let d = py_.post("/cli/mail/queue/drop", &body, line::QUEUE_DROP_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let id = or_env_bare(py_, &d, "id", "M_ID", line::QUEUE_DROP_CALL + 1)?;
    py_.print(&format!("dropped queued {id}"));
    Ok(())
}

fn acl_grant_py(py_: &mut Py) -> R<()> {
    let mut body = vec![
        ("mailbox", pyx::env_v(py_, "M_MAILBOX")),
        ("user", pyx::env_v(py_, "M_USER")),
    ];
    if !py_.env("M_RIGHTS").is_empty() {
        body.push(("rights", pyx::env_v(py_, "M_RIGHTS")));
    }
    for (env, key) in [
        ("M_ACL_READ", "read"),
        ("M_ACL_WRITE", "write"),
        ("M_ACL_POST", "post"),
        ("M_ACL_ADMIN", "admin"),
    ] {
        if py_.env(env) == "true" {
            body.push((key, Value::Bool(true)));
        }
    }
    let d = py_.post("/cli/mail/acl", &obj(body), line::ACL_GRANT_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::ACL_GRANT_CALL + 2;
    let user = or_env_bare(py_, &d, "user", "M_USER", l)?;
    let mailbox = or_env_bare(py_, &d, "mailbox", "M_MAILBOX", l)?;
    let rights = or_s(g(&d, "rights", l)?, "?");
    py_.print(&format!("acl grant {user} -> {mailbox} rights={rights}"));
    Ok(())
}

fn acl_revoke_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("mailbox", pyx::env_v(py_, "M_MAILBOX")),
        ("user", pyx::env_v(py_, "M_USER")),
    ]);
    let d = py_.post("/cli/mail/acl/revoke", &body, line::ACL_REVOKE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::ACL_REVOKE_CALL + 2;
    let user = or_env_bare(py_, &d, "user", "M_USER", l)?;
    let mailbox = or_env_bare(py_, &d, "mailbox", "M_MAILBOX", l)?;
    py_.print(&format!("acl revoke {user} from {mailbox}"));
    Ok(())
}

fn acl_show_py(py_: &mut Py) -> R<()> {
    let params = vec![("mailbox".to_string(), py_.env("M_MAILBOX").to_string())];
    let d = py_.get("/cli/mail/acl", &params, line::ACL_SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let shown = g(&d, "shareWith", line::ACL_SHOW_CALL + 1)?
        .filter(py::truthy)
        .unwrap_or_else(|| d.clone());
    py_.print(&dumps(&shown));
    Ok(())
}

/// The DNS records `autoconfig` prints, one a line.
fn records(py_: &mut Py, d: &Value, lf: u32, lr: u32) -> R<()> {
    for rec in items(g(d, "records", lf)?, lf)? {
        let typ = or_s(g(&rec, "type", lr)?, "?");
        let name = or_s(g(&rec, "name", lr)?, "?");
        let value = match g(&rec, "value", lr)?.filter(py::truthy) {
            Some(v) => py::str_of(&v),
            None => or_s(g(&rec, "display", lr)?, ""),
        };
        py_.print(&format!("{typ}  {name}  {value}"));
    }
    Ok(())
}

fn auto_show_py(py_: &mut Py) -> R<()> {
    let params = vec![("domain".to_string(), py_.env("M_DOMAIN").to_string())];
    let d = py_.get("/cli/mail/autoconfig", &params, line::AUTO_SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::AUTO_SHOW_CALL + 4;
    py_.print(&format!(
        "thunderbird  {}",
        or_s(g(&d, "thunderbirdUrl", l)?, "")
    ));
    py_.print(&format!(
        "autodiscover {}",
        or_s(g(&d, "autodiscoverUrl", l + 1)?, "")
    ));
    if let Some(u) = g(&d, "caldavUrl", l + 2)?.filter(py::truthy) {
        py_.print(&format!(
            "caldav       {}  (CalDAV/CardDAV: _caldavs/_carddavs SRV)",
            py::str_of(&u)
        ));
    }
    records(py_, &d, line::AUTO_SHOW_FOR, line::AUTO_SHOW_ROW)
}

fn auto_apply_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("domain", pyx::env_v(py_, "M_DOMAIN"))]);
    let d = py_.post("/cli/mail/autoconfig", &body, line::AUTO_APPLY_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::AUTO_APPLY_CALL + 4;
    if p::truthy(g(&d, "planted", l)?.as_ref()) {
        let dom = or_env_bare(py_, &d, "domain", "M_DOMAIN", l + 1)?;
        py_.print(&format!(
            "planted autoconfig records for {dom} via {CLI_NAME} dns (not A8 bind)"
        ));
        return Ok(());
    }
    let h = or_s(
        g(&d, "hint", l + 3)?,
        "print these records at your registrar:",
    );
    py_.print(&h);
    records(py_, &d, line::AUTO_APPLY_FOR, line::AUTO_APPLY_ROW)
}

/// `"%s" % v.get(key, "?")` at `l`.
fn q(v: &Value, key: &str, l: u32) -> R<String> {
    Ok(py::str_of(&p::get_or(v, key, p::sv("?")).at(l, MODULE)?))
}

fn active(v: Option<Value>) -> &'static str {
    if p::truthy(v.as_ref()) {
        "active"
    } else {
        "inactive"
    }
}

fn outside(v: Option<Value>) -> &'static str {
    if p::truthy(v.as_ref()) {
        " (outside)"
    } else {
        ""
    }
}

fn audit_py(py_: &mut Py) -> R<()> {
    let d = py_.call_full(
        Method::Get,
        "/cli/mail/sieve-audit",
        &[],
        None,
        Secs::Int(300),
        None,
        line::AUDIT_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    if !p::truthy(g(&d, "installed", line::AUDIT_CALL + 3)?.as_ref()) {
        py_.print("hosted mail is not installed here — nothing to audit");
        return Ok(());
    }
    let s = or_dict(g(&d, "summary", line::AUDIT_ENOTIFY - 2)?);
    let en = or_dict(g(&d, "enotify", line::AUDIT_ENOTIFY - 1)?);
    let le = line::AUDIT_ENOTIFY;
    let enotify = match g(&en, "disabled", le)? {
        Some(Value::Bool(true)) => "off".to_string(),
        Some(Value::Bool(false)) => "ON".to_string(),
        _ => format!("unknown ({})", or_s(g(&en, "error", le)?, "?")),
    };
    let l = line::AUDIT_SCANNED;
    let k2 = CLI_NAME;
    py_.print(&format!(
        "accounts scanned : {} ({} skipped: admin/service/group)",
        q(&s, "accountsScanned", l)?,
        q(&s, "accountsSkipped", l)?
    ));
    py_.print(&format!(
        "{k2} mailboxes     : {} ({} locked, {} NOT locked)",
        q(&s, "k2Mailboxes", l + 1)?,
        q(&s, "lockedMailboxes", l + 1)?,
        q(&s, "unlockedMailboxes", l + 1)?
    ));
    py_.print(&format!(
        "user scripts     : {}",
        q(&s, "userScripts", l + 2)?
    ));
    py_.print(&format!(
        "{k2}-named, other  : {}",
        q(&s, "unrecognisedK2Scripts", l + 3)?
    ));
    py_.print(&format!(
        "of those, outside: {} reach outside this box",
        q(&s, "scriptsWithOutsideTargets", l + 4)?
    ));
    py_.print(&format!(
        "{k2}'s own, no row : {} redirect(s) with no active forward behind them",
        q(&s, "k2ScriptRedirectsNoRow", l + 5)?
    ));
    py_.print(&format!("enotify          : {enotify}"));
    py_.print(&format!(
        "api key lock     : {} {PRODUCT_NAME} mailboxes locked, {} NOT locked (can still create API keys)",
        q(&s, "apiKeyLockedMailboxes", l + 7)?,
        q(&s, "apiKeyUnlockedMailboxes", l + 7)?
    ));
    py_.print(&format!(
        "api keys held    : {} on {} account(s)",
        q(&s, "apiKeys", l + 8)?,
        q(&s, "accountsWithApiKeys", l + 8)?
    ));
    if let Some(e) = g(&s, "errors", l + 9)?.filter(py::truthy) {
        py_.print(&format!("errors           : {}", py::str_of(&e)));
    }
    let accounts = g(&d, "accounts", line::AUDIT_ACCOUNTS)?;
    for a in items(accounts.clone(), line::AUDIT_ACCOUNTS)? {
        let lw = line::AUDIT_WHO;
        let who = match g(&a, "address", lw)?.filter(py::truthy) {
            Some(v) => py::str_of(&v),
            None => {
                let n = match g(&a, "name", lw)?.filter(py::truthy) {
                    Some(v) => py::str_of(&v),
                    None => s_of(g(&a, "accountId", lw)?),
                };
                format!("{n} (not a {PRODUCT_NAME} mailbox)")
            }
        };
        let lf = line::AUDIT_FLAGS;
        let mut flags = String::new();
        if p::truthy(g(&a, "k2Minted", lf)?.as_ref())
            && !p::truthy(g(&a, "sieveLocked", lf)?.as_ref())
        {
            flags.push_str("  [Sieve NOT locked]");
        }
        if p::truthy(g(&a, "k2Minted", lf + 2)?.as_ref())
            && matches!(g(&a, "apiKeyLocked", lf + 2)?, Some(Value::Bool(false)))
        {
            flags.push_str("  [API keys NOT locked]");
        }
        if let Some(n) = g(&a, "apiKeys", lf + 4)?.filter(py::truthy) {
            flags.push_str(&format!("  [{} API key(s)]", py::str_of(&n)));
        }
        py_.print("");
        py_.print(&format!("{who}{flags}"));
        if let Some(e) = g(&a, "apiKeysError", lf + 8)?.filter(py::truthy) {
            py_.print(&format!("  api keys: could not list ({})", py::str_of(&e)));
        }
        if let Some(e) = g(&a, "error", lf + 10)?.filter(py::truthy) {
            py_.print(&format!("  error: {}", py::str_of(&e)));
        }
        let lr = line::AUDIT_REDIRECT;
        for r in items(
            g(&a, "k2RedirectsNoRow", line::AUDIT_REDIRECTS)?,
            line::AUDIT_REDIRECTS,
        )? {
            let script = format!("\"{}\"", s_of(g(&r, "script", lr)?));
            let act = active(g(&r, "active", lr)?);
            let dom = s_of(g(&r, "domain", lr)?);
            let out = outside(g(&r, "external", lr)?);
            py_.print(&format!(
                "  {} {} redirect → {dom}{out}, no active forward behind it",
                pyx::ljust(&script, 24),
                pyx::ljust(act, 8)
            ));
        }
        for sc in items(g(&a, "scripts", line::AUDIT_SCRIPTS)?, line::AUDIT_SCRIPTS)? {
            let targets = items(
                g(&sc, "targets", line::AUDIT_TARGETS_OF)?,
                line::AUDIT_TARGETS_OF,
            )?;
            let mut parts = Vec::new();
            for t in targets {
                let gen = |key: &str| -> R<Option<Value>> {
                    attr_get(&t, key).map_err(|e| {
                        End::Raise(Tb {
                            frames: vec![
                                (line::AUDIT_TARGETS, MODULE),
                                (line::AUDIT_TARGETS, "<genexpr>"),
                            ],
                            exception: e,
                        })
                    })
                };
                let action = s_of(gen("action")?);
                let dom = s_of(gen("domain")?);
                let out = outside(gen("external")?);
                parts.push(format!("{action} → {dom}{out}"));
            }
            let tg = if parts.is_empty() {
                "no forward or notify".to_string()
            } else {
                parts.join(", ")
            };
            let ls = line::AUDIT_SCRIPT;
            let name = format!("\"{}\"", s_of(g(&sc, "name", ls)?));
            let act = active(g(&sc, "active", ls)?);
            let owner = s_of(g(&sc, "owner", ls)?);
            py_.print(&format!(
                "  {} {} {} {tg}",
                pyx::ljust(&name, 24),
                pyx::ljust(act, 8),
                pyx::ljust(&owner, 24)
            ));
        }
    }
    if !p::truthy(g(&d, "accounts", line::AUDIT_NONE)?.as_ref()) {
        py_.print("");
        py_.print(&format!("nothing found: no user Sieve scripts, no API keys, and every {PRODUCT_NAME} mailbox is locked"));
    } else if p::truthy(g(&s, "apiKeys", line::AUDIT_NONE + 3)?.as_ref()) {
        py_.print("");
        py_.print(&format!(
            "To revoke a mailbox's API keys: {CLI_NAME} hostmail password rotate <address>"
        ));
    }
    Ok(())
}
