//! A hosted mailbox's settings: `hostmail quota` (get, set), `hostmail
//! catchall` (set, unset, show) and `hostmail alias` (add, remove, list),
//! as their `cmd_hostmail_*` functions read the words
//! (`cli/src/hostmail-app-cert-backup.sh`) and `_mail_py`'s verbs answer
//! (`cli/src/mail.sh`).

use super::words::{scan, wants_help, Spec};
use crate::families::mail::core::{run_env, usage, At, Plan, Py, MODULE, R};
use crate::families::mail::py::{self as p, attr_get};
use crate::out::json::Value;
use crate::out::py;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["quota", "catchall", "alias"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const QUOTA_GET_CALL: u32 = 1124;
    pub const QUOTA_GET_Q: u32 = 1128;
    pub const QUOTA_GET_ADDR: u32 = 1130;
    pub const QUOTA_GET_DISK: u32 = 1131;
    pub const QUOTA_GET_MSGS: u32 = 1132;
    pub const QUOTA_SET_BYTES: u32 = 1137;
    pub const QUOTA_SET_GB: u32 = 1139;
    pub const QUOTA_SET_MSGS_INT: u32 = 1141;
    pub const QUOTA_SET_CALL: u32 = 1142;
    pub const QUOTA_SET_Q: u32 = 1146;
    pub const QUOTA_SET_ADDR: u32 = 1148;
    pub const QUOTA_SET_DISK: u32 = 1149;
    pub const QUOTA_SET_MSGS: u32 = 1150;
    pub const CATCHALL_GET_CALL: u32 = 1153;
    pub const CATCHALL_GET_ADDR: u32 = 1157;
    pub const CATCHALL_GET_DOMAIN: u32 = 1158;
    pub const CATCHALL_SET_CALL: u32 = 1163;
    pub const CATCHALL_SET_ADDR: u32 = 1167;
    pub const CATCHALL_SET_DOMAIN: u32 = 1168;
    pub const ALIAS_LIST_CALL: u32 = 1172;
    pub const ALIAS_LIST_ROWS: u32 = 1176;
    pub const ALIAS_LIST_NONE: u32 = 1178;
    pub const ALIAS_LIST_PRINT: u32 = 1180;
    pub const ALIAS_ADD_CALL: u32 = 1184;
    pub const ALIAS_ADD_PRINT: u32 = 1188;
    pub const ALIAS_REMOVE_CALL: u32 = 1192;
    pub const ALIAS_REMOVE_PRINT: u32 = 1196;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "quota" => quota(words),
        "catchall" => catchall(words),
        "alias" => alias(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// `cmd_hostmail_quota`.
fn quota(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/quota"));
    };
    match *sub {
        "get" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/quota/get"));
            }
            let g = scan(
                rest,
                &Spec {
                    slots: &["address"],
                    values: &[("--address", "address", "--address requires a value")],
                    switches: &[],
                    unknown: &format!(
                        "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail quota get <addr> [--json]"
                    ),
                    extra: "unexpected extra argument '{w}' — one address only",
                },
            )?;
            if g.get("address").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail quota get <addr>")));
            }
            Ok(Plan::Run(
                run_env("quota_get", g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "set" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/quota/set"));
            }
            let use_ = format!("{CLI_NAME} hostmail quota set <addr> --bytes N|--gb N [--messages N]");
            let g = scan(
                rest,
                &Spec {
                    slots: &["address"],
                    values: &[
                        ("--bytes", "bytes", "--bytes requires a number (0 = unlimited)"),
                        ("--gb", "gb", "--gb requires a number (0 = unlimited)"),
                        (
                            "--messages",
                            "messages",
                            "--messages requires a number (0 = unlimited)",
                        ),
                        ("--address", "address", "--address requires a value"),
                    ],
                    switches: &[],
                    unknown: &format!("unknown flag '{{w}}' — usage: {use_}"),
                    extra: "unexpected extra argument '{w}' — one address only",
                },
            )?;
            let (bytes, gb, messages) = (g.get("bytes"), g.get("gb"), g.get("messages"));
            if g.get("address").is_empty() {
                return Err(usage(use_));
            }
            if !bytes.is_empty() && !gb.is_empty() {
                return Err(usage("give either --bytes or --gb, not both"));
            }
            if bytes.is_empty() && gb.is_empty() && messages.is_empty() {
                return Err(usage(
                    "give --bytes N or --gb N and/or --messages N (0 = unlimited)",
                ));
            }
            for (v, flag) in [(bytes, "--bytes"), (gb, "--gb"), (messages, "--messages")] {
                if !v.is_empty() && !crate::families::mail::core::is_uint(v) {
                    return Err(usage(format!(
                        "{flag} must be a non-negative number (0 = unlimited)"
                    )));
                }
            }
            Ok(Plan::Run(
                run_env("quota_set", g.json)
                    .with("M_ADDRESS", g.get("address"))
                    .with("M_QUOTA_BYTES", bytes)
                    .with("M_QUOTA_GB", gb)
                    .with("M_QUOTA_MESSAGES", messages),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/quota")),
        other => Err(usage(format!(
            "unknown quota command '{other}' — usage: {CLI_NAME} hostmail quota <get|set>"
        ))),
    }
}

/// `cmd_hostmail_catchall`.
fn catchall(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/catchall"));
    };
    let (page, use_) = match *sub {
        "set" => (
            "hostmail/catchall/set",
            format!("{CLI_NAME} hostmail catchall set <domain> --address <addr> [--json]"),
        ),
        "unset" => (
            "hostmail/catchall/unset",
            format!("{CLI_NAME} hostmail catchall unset <domain> [--json]"),
        ),
        "show" => (
            "hostmail/catchall/show",
            format!("{CLI_NAME} hostmail catchall show <domain> [--json]"),
        ),
        "--help" | "-h" => return Ok(Plan::Help("hostmail/catchall")),
        other => {
            return Err(usage(format!(
                "unknown catchall command '{other}' — usage: {CLI_NAME} hostmail catchall <set|unset|show>"
            )))
        }
    };
    if wants_help(rest) {
        return Ok(Plan::Help(page));
    }
    let set = *sub == "set";
    let values: &[(&str, &str, &str)] = if set {
        &[
            ("--address", "address", "--address requires a minted mailbox"),
            ("--domain", "domain", "--domain requires a value"),
        ]
    } else {
        &[("--domain", "domain", "--domain requires a value")]
    };
    let g = scan(
        rest,
        &Spec {
            slots: &["domain"],
            values,
            switches: &[],
            unknown: &format!("unknown flag '{{w}}' — usage: {use_}"),
            extra: "unexpected extra argument '{w}'",
        },
    )?;
    let domain = g.get("domain");
    if domain.is_empty() {
        let short = use_.strip_suffix(" [--json]").unwrap_or(&use_);
        return Err(usage(short.to_string()));
    }
    if set && g.get("address").is_empty() {
        return Err(usage(
            "catchall set requires --address <minted mailbox on that domain>",
        ));
    }
    let verb = if *sub == "show" {
        "catchall_get"
    } else {
        "catchall_set"
    };
    let mut env = run_env(verb, g.json).with("M_DOMAIN", domain);
    if *sub != "show" {
        env.set("M_ADDRESS", g.get("address"));
    }
    Ok(Plan::Run(env))
}

/// `cmd_hostmail_alias`.
fn alias(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/alias"));
    };
    let (page, verb) = match *sub {
        "add" => ("hostmail/alias/add", "alias_add"),
        "remove" => ("hostmail/alias/remove", "alias_remove"),
        "list" => ("hostmail/alias/list", "alias_list"),
        "--help" | "-h" => return Ok(Plan::Help("hostmail/alias")),
        other => {
            return Err(usage(format!(
                "unknown alias command '{other}' — usage: {CLI_NAME} hostmail alias <add|remove|list>"
            )))
        }
    };
    if wants_help(rest) {
        return Ok(Plan::Help(page));
    }
    let list = *sub == "list";
    let usage_line = if list {
        format!("{CLI_NAME} hostmail alias list <mailbox> [--json]")
    } else {
        format!("{CLI_NAME} hostmail alias {sub} <mailbox> <alias> [--json]")
    };
    let g = scan(
        rest,
        &Spec {
            slots: if list {
                &["mailbox"]
            } else {
                &["mailbox", "alias"]
            },
            values: &[],
            switches: &[],
            unknown: &format!("unknown flag '{{w}}' — usage: {usage_line}"),
            extra: "unexpected extra argument '{w}'",
        },
    )?;
    let (mailbox, alias) = (g.get("mailbox"), g.get("alias"));
    if list {
        if mailbox.is_empty() {
            return Err(usage(format!("{CLI_NAME} hostmail alias list <mailbox>")));
        }
        return Ok(Plan::Run(
            run_env(verb, g.json).with("M_MAILBOX", mailbox),
        ));
    }
    if mailbox.is_empty() || alias.is_empty() {
        return Err(usage(if *sub == "add" {
            format!("{CLI_NAME} hostmail alias add <mailbox> <alias> — extra address on an existing mailbox (not a mint; {CLI_NAME} hostmail create mints)")
        } else {
            format!("{CLI_NAME} hostmail alias remove <mailbox> <alias>")
        }));
    }
    Ok(Plan::Run(
        run_env(verb, g.json)
            .with("M_MAILBOX", mailbox)
            .with("M_ALIAS", alias),
    ))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "quota_get" => quota_py(py_, false),
        "quota_set" => quota_py(py_, true),
        "catchall_get" => catchall_py(py_, false),
        "catchall_set" => catchall_py(py_, true),
        "alias_list" => alias_list_py(py_),
        "alias_add" | "alias_remove" => alias_change_py(py_, verb == "alias_add"),
        _ => return None,
    })
}

fn g(d: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(d, key).at(l, MODULE)
}

fn sv(t: &str) -> Value {
    p::sv(t)
}

/// `d.get(key) or env(k) or "?"` at `l`, as `%s` prints it.
fn or_env(py_: &Py, d: &Value, key: &str, env_key: &str, l: u32) -> R<String> {
    Ok(match g(d, key, l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None if !py_.env(env_key).is_empty() => py_.env(env_key).to_string(),
        None => "?".into(),
    })
}

/// `int(env(k))`, raised at `l`.
fn int_env(py_: &Py, key: &str, l: u32) -> R<Value> {
    let digits = p::py_int(&sv(py_.env(key))).at(l, MODULE)?;
    Ok(Value::Int(digits))
}

fn quota_py(py_: &mut Py, set: bool) -> R<()> {
    let d = if set {
        let mut body = vec![("address", sv(py_.env("M_ADDRESS")))];
        for (env_key, key, l) in [
            ("M_QUOTA_BYTES", "bytes", line::QUOTA_SET_BYTES),
            ("M_QUOTA_GB", "gb", line::QUOTA_SET_GB),
            ("M_QUOTA_MESSAGES", "messages", line::QUOTA_SET_MSGS_INT),
        ] {
            if !py_.env(env_key).is_empty() {
                body.push((key, int_env(py_, env_key, l)?));
            }
        }
        py_.post(
            "/cli/mail/quota",
            &crate::families::mail::core::obj(body),
            line::QUOTA_SET_CALL,
        )?
    } else {
        let params = vec![("address".to_string(), py_.env("M_ADDRESS").to_string())];
        py_.get("/cli/mail/quota", &params, line::QUOTA_GET_CALL)?
    };
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let (lq, la, ld, lm) = if set {
        (
            line::QUOTA_SET_Q,
            line::QUOTA_SET_ADDR,
            line::QUOTA_SET_DISK,
            line::QUOTA_SET_MSGS,
        )
    } else {
        (
            line::QUOTA_GET_Q,
            line::QUOTA_GET_ADDR,
            line::QUOTA_GET_DISK,
            line::QUOTA_GET_MSGS,
        )
    };
    let q = p::or(g(&d, "quotas", lq)?, Value::Object(Vec::new()));
    let a = or_env(py_, &d, "address", "M_ADDRESS", la)?;
    let field = |k: &str, l: u32| -> R<String> {
        Ok(match g(&q, k, l)? {
            Some(Value::Null) | None => "?".into(),
            Some(v) => py::str_of(&v),
        })
    };
    let disk = field("maxDiskQuota", ld)?;
    let msgs = field("maxEmails", lm)?;
    let head = if set { "quota set for " } else { "" };
    py_.print(&format!("{head}{a}  disk={disk}  messages={msgs}"));
    Ok(())
}

fn catchall_py(py_: &mut Py, set: bool) -> R<()> {
    let d = if set {
        let body = crate::families::mail::core::obj(vec![
            ("domain", sv(py_.env("M_DOMAIN"))),
            ("address", sv(py_.env("M_ADDRESS"))),
        ]);
        py_.post("/cli/mail/catchall", &body, line::CATCHALL_SET_CALL)?
    } else {
        let params = vec![("domain".to_string(), py_.env("M_DOMAIN").to_string())];
        py_.get("/cli/mail/catchall", &params, line::CATCHALL_GET_CALL)?
    };
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let (la, ld) = if set {
        (line::CATCHALL_SET_ADDR, line::CATCHALL_SET_DOMAIN)
    } else {
        (line::CATCHALL_GET_ADDR, line::CATCHALL_GET_DOMAIN)
    };
    let addr = g(&d, "catchAllAddress", la)?;
    let domain = or_env(py_, &d, "domain", "M_DOMAIN", ld)?;
    let shown = match addr.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => "none".into(),
    };
    py_.print(&format!("{domain}  catch-all={shown}"));
    Ok(())
}

/// `env("M_MAILBOX") or env("M_ADDRESS")`.
fn mailbox(py_: &Py) -> String {
    let m = py_.env("M_MAILBOX");
    if m.is_empty() {
        py_.env("M_ADDRESS").to_string()
    } else {
        m.to_string()
    }
}

fn alias_list_py(py_: &mut Py) -> R<()> {
    let params = vec![("mailbox".to_string(), mailbox(py_))];
    let d = py_.get("/cli/mail/alias", &params, line::ALIAS_LIST_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(g(&d, "aliases", line::ALIAS_LIST_ROWS)?, Value::Array(Vec::new()));
    if !py::truthy(&rows) {
        let m = or_env(py_, &d, "mailbox", "M_MAILBOX", line::ALIAS_LIST_NONE)?;
        py_.print(&format!("{m}  aliases=none"));
    } else {
        let l = line::ALIAS_LIST_PRINT;
        let m = p::or(g(&d, "mailbox", l)?, sv("?"));
        let joined = p::join(", ", &rows).at(l, MODULE)?;
        py_.print(&format!("{}  aliases={joined}", py::str_of(&m)));
    }
    Ok(())
}

fn alias_change_py(py_: &mut Py, add: bool) -> R<()> {
    let body = crate::families::mail::core::obj(vec![
        ("mailbox", sv(&mailbox(py_))),
        ("alias", sv(py_.env("M_ALIAS"))),
    ]);
    let (route, lc, lp) = if add {
        ("/cli/mail/alias", line::ALIAS_ADD_CALL, line::ALIAS_ADD_PRINT)
    } else {
        (
            "/cli/mail/alias/remove",
            line::ALIAS_REMOVE_CALL,
            line::ALIAS_REMOVE_PRINT,
        )
    };
    let d = py_.post(route, &body, lc)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let first = p::or(
        g(&d, if add { "alias" } else { "removed" }, lp)?,
        sv(py_.env("M_ALIAS")),
    );
    let m = p::or(g(&d, "mailbox", lp)?, sv("?"));
    let (first, m) = (py::str_of(&first), py::str_of(&m));
    py_.print(&if add {
        format!("alias {first} -> {m}")
    } else {
        format!("removed alias {first} from {m}")
    });
    Ok(())
}
