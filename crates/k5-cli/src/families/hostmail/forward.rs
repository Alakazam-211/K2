//! `hostmail forward`: a hosted mailbox's forwards, for the owner, an
//! Admin or an agent that manages hosted mail (the `/cli/mail/forward*`
//! routes), as `cmd_hostmail_forward` reads the words. The Python is the
//! branch `mail forward` shares ([`crate::families::mail::forward`]).

use crate::families::mail::core::{run_env, usage, Plan};
use crate::families::mail::forward::{parse_set, wants_help, Family};
use k2_client::brand::CLI_NAME;

/// `cmd_hostmail_forward`: the words after `hostmail forward`.
pub fn parse(words: &[&str]) -> Plan {
    const HELP: Plan = Plan::Help("hostmail/forward");
    let Some((sub, rest)) = words.split_first() else {
        return HELP;
    };
    let r = match *sub {
        "set" | "add" | "rm" => {
            if wants_help(rest) {
                return Plan::Help(page(sub));
            }
            parse_set(Family::Hostmail, sub, rest, &format!("forward_{sub}")).map(Plan::Run)
        }
        "show" | "unset" | "pause" | "resume" => {
            if wants_help(rest) {
                return Plan::Help(page(sub));
            }
            simple(sub, rest)
        }
        "list" => {
            if wants_help(rest) {
                return Plan::Help(page(sub));
            }
            list(rest)
        }
        "--help" | "-h" => return HELP,
        other => Err(usage(format!(
            "unknown forward command '{other}' — usage: {CLI_NAME} hostmail forward <set|add|rm|show|unset|pause|resume|list>"
        ))),
    };
    r.unwrap_or_else(|p| p)
}

/// `cmd_help_hostmail_forward_<op>`'s page.
fn page(op: &str) -> &'static str {
    match op {
        "set" => "hostmail/forward/set",
        "add" => "hostmail/forward/add",
        "rm" => "hostmail/forward/rm",
        "show" => "hostmail/forward/show",
        "unset" => "hostmail/forward/unset",
        "pause" => "hostmail/forward/pause",
        "resume" => "hostmail/forward/resume",
        _ => "hostmail/forward/list",
    }
}

/// `cmd_hostmail_forward_simple`: `show|unset|pause|resume <addr>
/// [--to <dest>] [--json]`.
fn simple(op: &str, words: &[&str]) -> Result<Plan, Plan> {
    let use_ = match op {
        "pause" | "resume" => {
            format!("{CLI_NAME} hostmail forward {op} <addr> [--to <dest>] [--json]")
        }
        _ => format!("{CLI_NAME} hostmail forward {op} <addr> [--json]"),
    };
    let (mut address, mut json, mut to) = (String::new(), false, String::new());
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => json = true,
            "--address" => {
                address = words
                    .get(i + 1)
                    .ok_or_else(|| usage("--address requires a value"))?
                    .to_string();
                i += 1;
            }
            "--to" => {
                if !matches!(op, "pause" | "resume") {
                    return Err(usage(format!("unknown flag '--to' — usage: {use_}")));
                }
                to = words
                    .get(i + 1)
                    .ok_or_else(|| usage(format!("--to requires an address — usage: {use_}")))?
                    .to_string();
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(format!("unknown flag '{f}' — usage: {use_}")));
            }
            w => {
                if address.is_empty() {
                    address = w.to_string();
                } else {
                    return Err(usage(format!(
                        "unexpected extra argument '{w}' — usage: {use_}"
                    )));
                }
            }
        }
        i += 1;
    }
    if address.is_empty() {
        return Err(usage(use_));
    }
    let verb = if op == "show" {
        "forward_get".to_string()
    } else {
        format!("forward_{op}")
    };
    Ok(Plan::Run(
        run_env(&verb, json)
            .with("M_ADDRESS", address)
            .with("M_TO", to),
    ))
}

/// `cmd_hostmail_forward_list`: `[--paused|--active|--pending]
/// [--external] [--json]`.
fn list(words: &[&str]) -> Result<Plan, Plan> {
    let (mut state, mut kind, mut json) = ("", "", false);
    for w in words {
        match *w {
            "--json" => json = true,
            "--paused" => state = "paused",
            "--active" => state = "active",
            "--pending" => state = "pending_verify",
            "--external" => kind = "external",
            other => {
                return Err(usage(format!(
                    "unknown argument '{other}' — usage: {CLI_NAME} hostmail forward list [--paused|--active|--pending] [--external] [--json]"
                )))
            }
        }
    }
    Ok(Plan::Run(
        run_env("forward_list", json)
            .with("M_STATE", state)
            .with("M_KIND", kind),
    ))
}
