//! The mail server and its credentials: `hostmail enable`, `disable`,
//! `uninstall`, `upgrade`, `rotate-admin`, `password` (rotate, keep),
//! `app-password` (add, list, revoke, keep), `person` (and `person link`)
//! and `import`, as their `cmd_hostmail_*` functions read the words
//! (`cli/src/hostmail-app-cert-backup.sh`) and `_mail_py`'s verbs answer
//! (`cli/src/mail.sh`).
//!
//! `import --pass-stdin` reads the password where the loop meets the
//! flag, as bash's `$(cat)` does, and it goes out only in the request
//! body: no child starts.

use super::pyx::{self, addr, g, hint_or, items, or_dict, or_s, s_of, sv};
use super::words::{scan, wants_help, Spec};
use crate::exit::Exit;
use crate::families::mail::core::{
    obj, py_float, raise_at, run_env, usage, At, End, Plan, Py, Secs, Tb, Within, MODULE, R,
};
use crate::families::mail::py as p;
use crate::out::json::Value;
use crate::out::{self, py};
use crate::transport::urllib::Method;
use k2_client::brand::CLI_NAME;
use k2_client::{env_names, product_name};
use std::time::{Duration, Instant};

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &[
    "enable",
    "disable",
    "uninstall",
    "upgrade",
    "rotate-admin",
    "password",
    "app-password",
    "person",
    "import",
];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const ENABLE_CALL: u32 = 2126;
    pub const ENABLE_ALREADY: u32 = 2129;
    pub const ENABLE_HELPER: u32 = 2134;
    pub const ENABLE_HINT: u32 = 2136;
    pub const IMPORT_PORT: u32 = 2151;
    pub const IMPORT_CALL: u32 = 2155;
    pub const IMPORT_PRINT: u32 = 2159;
    pub const DISABLE_CALL: u32 = 2163;
    pub const DISABLE_PRINT: u32 = 2164;
    pub const UNINSTALL_CALL: u32 = 2170;
    pub const UNINSTALL_PRINT: u32 = 2172;
    pub const HB_FLOAT: u32 = 2182;
    pub const HB_FORMAT: u32 = 2185;
    pub const ACK_CALL: u32 = 2188;
    pub const ACK_PRINT: u32 = 2189;
    pub const DRY_CALL: u32 = 2191;
    pub const DRY_BLOCKERS: u32 = 2202;
    pub const DRY_BLOCKER: u32 = 2203;
    pub const DRY_HB: u32 = 2206;
    pub const DRY_HB_FREE: u32 = 2207;
    pub const DRY_OUTAGE: u32 = 2211;
    pub const DRY_ASSUMPTION: u32 = 2212;
    pub const DRY_PLAN: u32 = 2214;
    pub const DRY_ROLLBACK: u32 = 2217;
    pub const DRY_NOTES: u32 = 2219;
    pub const UP_CALL: u32 = 2224;
    pub const UP_OUTAGE: u32 = 2232;
    pub const UP_SLEEP: u32 = 2250;
    pub const UP_LOST: u32 = 2254;
    pub const UP_UPGRADE: u32 = 2264;
    pub const UP_STEPS: u32 = 2265;
    pub const UP_STEP_FOR: u32 = 2267;
    pub const UP_STEP: u32 = 2268;
    pub const UP_SEEN: u32 = 2270;
    pub const UP_MANUAL: u32 = 2291;
    pub const PERSON_LINK_CALL: u32 = 1453;
    pub const ROTATE_ADMIN_CALL: u32 = 2551;
    pub const PW_CALL: u32 = 2558;
    pub const PW_EP_CALL: u32 = 2572;
    pub const PW_EP_SUBMIT: u32 = 2573;
    pub const PW_EP_JMAP: u32 = 2574;
    pub const PW_EP_HOST: u32 = 2568;
    pub const APW_LIST_CALL: u32 = 2652;
    pub const APW_LIST_LEN: u32 = 2660;
    pub const APW_LIST_FOR: u32 = 2661;
    pub const APW_LIST_ROW: u32 = 2662;
    pub const APW_ADD_CALL: u32 = 2668;
    pub const APW_REVOKE_CALL: u32 = 2679;
    pub const KEEP_CALL: u32 = 2690;
    pub const PERSON_ALL_CALL: u32 = 2702;
    pub const PERSON_ALL_LEN: u32 = 2709;
    pub const PERSON_ALL_FOR: u32 = 2710;
    pub const PERSON_ALL_JOIN: u32 = 2714;
    pub const PERSON_CALL: u32 = 2718;
    pub const KEEP_ALL_CALL: u32 = 2732;
    pub const KEEP_ALL_KEPT: u32 = 2739;
    pub const KEEP_ALL_NEWER: u32 = 2743;
    pub const KEEP_ALL_UNDATED: u32 = 2744;
    pub const KEEP_ALL_ERRORS: u32 = 2748;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "enable" => enable(words),
        "disable" => only_json(words, "hostmail/disable", "server_disable", "disable"),
        "rotate-admin" => only_json(
            words,
            "hostmail/rotate-admin",
            "rotate_admin",
            "rotate-admin",
        ),
        "uninstall" => uninstall(words),
        "upgrade" => upgrade(words),
        "password" => password(words),
        "app-password" => app_password(words),
        "person" => person(words),
        "import" => import(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// `cmd_hostmail_enable`.
fn enable(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/enable"));
    }
    let use_ = format!("usage: {CLI_NAME} hostmail enable --hostname <name> [--json]");
    let g = scan(
        words,
        &Spec {
            slots: &[],
            values: &[(
                "--hostname",
                "hostname",
                &format!("--hostname requires a value — {use_}"),
            )],
            switches: &[],
            unknown: &format!("unexpected argument '{{w}}' — {use_}"),
            extra: &format!("unexpected argument '{{w}}' — {use_}"),
        },
    )?;
    if g.get("hostname").is_empty() {
        return Err(usage(format!(
            "{CLI_NAME} hostmail enable --hostname <name> (e.g. mail.acme.dev)"
        )));
    }
    Ok(Plan::Run(
        run_env("server_enable", g.json).with("M_HOSTNAME", g.get("hostname")),
    ))
}

/// `cmd_hostmail_disable` and `cmd_hostmail_rotate_admin`: `--json` only.
fn only_json(words: &[&str], page: &'static str, verb: &str, name: &str) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help(page));
    }
    let bad = format!("unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail {name} [--json]");
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
    Ok(Plan::Run(run_env(verb, g.json)))
}

/// `cmd_hostmail_uninstall`.
fn uninstall(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/uninstall"));
    }
    let short = format!("{CLI_NAME} hostmail uninstall --purge --confirm-hostname <host>");
    let bad = format!("unexpected argument '{{w}}' — usage: {short} [--json]");
    let g = scan(
        words,
        &Spec {
            slots: &[],
            values: &[(
                "--confirm-hostname",
                "hostname",
                &format!("--confirm-hostname requires a value — usage: {short}"),
            )],
            switches: &[("--purge", "purge")],
            unknown: &bad,
            extra: &bad,
        },
    )?;
    if !g.on("purge") || g.get("hostname").is_empty() {
        return Err(usage(format!(
            "{short} (both flags required; nothing was sent)"
        )));
    }
    Ok(Plan::Run(
        run_env("server_uninstall", g.json).with("M_HOSTNAME", g.get("hostname")),
    ))
}

/// `cmd_hostmail_upgrade`.
fn upgrade(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/upgrade"));
    }
    let bad = format!(
        "unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail upgrade [--dry-run] [--json] | {CLI_NAME} hostmail upgrade --acknowledge-failed [--json]"
    );
    let g = scan(
        words,
        &Spec {
            slots: &[],
            values: &[],
            switches: &[("--dry-run", "dry"), ("--acknowledge-failed", "ack")],
            unknown: &bad,
            extra: &bad,
        },
    )?;
    if g.on("dry") && g.on("ack") {
        return Err(usage(
            "--dry-run and --acknowledge-failed are separate calls (nothing was sent)",
        ));
    }
    let tf = |b: bool| if b { "true" } else { "false" };
    Ok(Plan::Run(
        run_env("server_upgrade", g.json)
            .with("M_DRY_RUN", tf(g.on("dry")))
            .with("M_ACK_FAILED", tf(g.on("ack"))),
    ))
}

/// One `<addr>` and `--json`, for `password rotate`, `app-password
/// list` and the like: `unknown` for a flag, `extra` for a second word.
fn one_addr<'a>(
    words: &[&str],
    values: &'a [(&'static str, &'static str, &'a str)],
    switches: &'a [(&'static str, &'static str)],
    slots: &'a [&'static str],
    unknown: &str,
    extra: &str,
) -> Result<super::words::Got, Plan> {
    scan(
        words,
        &Spec {
            slots,
            values,
            switches,
            unknown,
            extra,
        },
    )
}

/// `cmd_hostmail_password`.
fn password(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/password"));
    };
    match *sub {
        "rotate" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/password/rotate"));
            }
            let g = one_addr(
                rest,
                &[],
                &[],
                &["address"],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail password rotate <addr> [--json]"
                ),
                "unexpected extra argument '{w}' — one <addr> only",
            )?;
            if g.get("address").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail password rotate <addr> (hosted address)"
                )));
            }
            Ok(Plan::Run(
                run_env("password_rotate", g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "keep" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/password/keep"));
            }
            let g = one_addr(
                rest,
                &[],
                &[("--all-existing", "all")],
                &["address"],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail password keep <addr>|--all-existing [--json]"
                ),
                "unexpected extra argument '{w}' — one <addr> only",
            )?;
            let a = g.get("address");
            if g.on("all") {
                if !a.is_empty() {
                    return Err(usage(format!(
                        "--all-existing covers every mailbox — drop '{a}'"
                    )));
                }
                return Ok(Plan::Run(run_env("credentials_keep_all_mailbox", g.json)));
            }
            if a.is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail password keep <addr>|--all-existing (hosted address)"
                )));
            }
            Ok(Plan::Run(
                run_env("credentials_keep", g.json)
                    .with("M_ADDRESS", a)
                    .with("M_ID", ""),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/password")),
        other => Err(usage(format!(
            "unknown password command '{other}' — usage: {CLI_NAME} hostmail password <rotate|keep> <addr> [--json]"
        ))),
    }
}

/// `cmd_hostmail_app_password`.
fn app_password(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/app-password"));
    };
    let one = "unexpected extra argument '{w}' — one <addr> only";
    let two = "unexpected extra argument '{w}' — <addr> <id> only";
    match *sub {
        "add" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/app-password/add"));
            }
            let use_ = format!("{CLI_NAME} hostmail app-password add <addr> [--label <s>] [--json]");
            let g = one_addr(
                rest,
                &[("--label", "label", "--label requires a value")],
                &[],
                &["address"],
                &format!("unknown flag '{{w}}' — usage: {use_}"),
                one,
            )?;
            if g.get("address").is_empty() {
                return Err(usage(use_));
            }
            Ok(Plan::Run(
                run_env("app_password_add", g.json)
                    .with("M_ADDRESS", g.get("address"))
                    .with("M_LABEL", g.get("label")),
            ))
        }
        "list" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/app-password/list"));
            }
            let use_ = format!("{CLI_NAME} hostmail app-password list <addr> [--json]");
            let g = one_addr(
                rest,
                &[],
                &[],
                &["address"],
                &format!("unknown flag '{{w}}' — usage: {use_}"),
                one,
            )?;
            if g.get("address").is_empty() {
                return Err(usage(use_));
            }
            Ok(Plan::Run(
                run_env("app_password_list", g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "revoke" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/app-password/revoke"));
            }
            let use_ = format!("{CLI_NAME} hostmail app-password revoke <addr> <id> [--json]");
            let g = one_addr(
                rest,
                &[],
                &[],
                &["address", "id"],
                &format!("unknown flag '{{w}}' — usage: {use_}"),
                two,
            )?;
            if g.get("address").is_empty() || g.get("id").is_empty() {
                return Err(usage(use_));
            }
            Ok(Plan::Run(
                run_env("app_password_revoke", g.json)
                    .with("M_ADDRESS", g.get("address"))
                    .with("M_ID", g.get("id")),
            ))
        }
        "keep" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/app-password/keep"));
            }
            let g = one_addr(
                rest,
                &[],
                &[("--all-existing", "all")],
                &["address", "id"],
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail app-password keep <addr> <id>|--all-existing [--json]"
                ),
                two,
            )?;
            let a = g.get("address");
            if g.on("all") {
                if !a.is_empty() {
                    return Err(usage(format!(
                        "--all-existing covers every mailbox — drop '{a}'"
                    )));
                }
                return Ok(Plan::Run(run_env(
                    "credentials_keep_all_app_password",
                    g.json,
                )));
            }
            if a.is_empty() || g.get("id").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail app-password keep <addr> <id> [--json]"
                )));
            }
            Ok(Plan::Run(
                run_env("credentials_keep", g.json)
                    .with("M_ADDRESS", a)
                    .with("M_ID", g.get("id")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/app-password")),
        other => Err(usage(format!(
            "unknown app-password command '{other}' — usage: {CLI_NAME} hostmail app-password <add|list|revoke|keep>"
        ))),
    }
}

/// `cmd_hostmail_person` (and `cmd_hostmail_person_link`).
fn person(words: &[&str]) -> Result<Plan, Plan> {
    if words.first() == Some(&"link") {
        return person_link(&words[1..]);
    }
    if words.is_empty() || wants_help(words) {
        return Ok(Plan::Help("hostmail/person"));
    }
    let use_ = format!(
        "{CLI_NAME} hostmail person <addr> on|off [--json] | {CLI_NAME} hostmail person --all --workspace <ws> on|off [--json]"
    );
    let (mut address, mut state, mut json, mut all, mut ws) =
        (String::new(), String::new(), false, false, String::new());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => json = true,
            "--all" => all = true,
            "--workspace" => {
                ws = words
                    .get(i + 1)
                    .ok_or_else(|| usage("--workspace requires a value"))?
                    .to_string();
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(format!("unknown flag '{f}' — usage: {use_}")));
            }
            _ => {
                if all && state.is_empty() && (w == "on" || w == "off") {
                    state = w.into();
                } else if address.is_empty() && !all {
                    address = w.into();
                } else if state.is_empty() {
                    state = w.into();
                } else {
                    return Err(usage(format!(
                        "unexpected extra argument '{w}' — usage: {use_}"
                    )));
                }
            }
        }
        i += 1;
    }
    if state != "on" && state != "off" {
        return Err(usage(use_));
    }
    if all {
        if !address.is_empty() || ws.is_empty() {
            return Err(usage(format!(
                "--all needs --workspace <ws> and no <addr> — usage: {use_}"
            )));
        }
    } else if address.is_empty() || !ws.is_empty() {
        return Err(usage(use_));
    }
    Ok(Plan::Run(
        run_env("person_set", json)
            .with("M_ADDRESS", address)
            .with("M_PERSON", state)
            .with("M_ALL", if all { "true" } else { "false" })
            .with("M_WORKSPACE", ws),
    ))
}

/// `cmd_hostmail_person_link`.
fn person_link(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/person/link"));
    }
    let use_ = format!(
        "{CLI_NAME} hostmail person link <addr> <login|owner> [--json] | {CLI_NAME} hostmail person link <addr> --unlink [--json]"
    );
    let g = scan(
        words,
        &Spec {
            slots: &["address", "person"],
            values: &[],
            switches: &[("--unlink", "unlink")],
            unknown: &format!("unknown flag '{{w}}' — usage: {use_}"),
            extra: &format!("unexpected extra argument '{{w}}' — usage: {use_}"),
        },
    )?;
    let (a, who) = (g.get("address"), g.get("person"));
    if a.is_empty() {
        return Err(usage(use_));
    }
    if g.on("unlink") && !who.is_empty() {
        return Err(usage(format!(
            "give a login or --unlink, not both — usage: {use_}"
        )));
    }
    if !g.on("unlink") && who.is_empty() {
        return Err(usage(format!(
            "name the login ({CLI_NAME} users list) or owner, or --unlink — usage: {use_}"
        )));
    }
    Ok(Plan::Run(
        run_env("person_link", g.json)
            .with("M_ADDRESS", a)
            .with("M_PERSON", who),
    ))
}

/// `cmd_hostmail_import`: the password comes from stdin where the loop
/// meets `--pass-stdin`, as `$(cat)` takes it.
fn import(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/import"));
    }
    let mut v: [String; 5] = Default::default(); // address maildir host user port
    let mut pass = String::new();
    let (mut json, mut skip) = (false, false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let slot = match w {
            "--address" => Some((0, "--address requires a value")),
            "--maildir" => Some((1, "--maildir requires a path")),
            "--imapsync" => Some((2, "--imapsync requires from=<host>")),
            "--user" => Some((3, "--user requires a value")),
            "--port" => Some((4, "--port requires a port number")),
            _ => None,
        };
        if let Some((k, missing)) = slot {
            let val = words.get(i + 1).ok_or_else(|| usage(missing))?;
            v[k] = if k == 2 {
                val.strip_prefix("from=").unwrap_or(val).to_string()
            } else {
                val.to_string()
            };
            i += 2;
            continue;
        }
        match w {
            "--pass-stdin" => {
                pass = super::stdin_all();
                if pass.is_empty() {
                    return Err(usage("--pass-stdin: no password on stdin"));
                }
            }
            "--skip-inbox" => skip = true,
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Err(usage(format!(
                    "unknown flag '{f}' — usage: {CLI_NAME} hostmail import --address <a@domain> (--maildir <path> | --imapsync from=<host> --user <u> --pass-stdin) [--skip-inbox] [--json]"
                )));
            }
            _ => {
                if v[0].is_empty() {
                    v[0] = w.into();
                } else {
                    return Err(usage(format!("unexpected extra argument '{w}'")));
                }
            }
        }
        i += 1;
    }
    if v[0].is_empty() {
        return Err(usage("import requires --address <a@domain>"));
    }
    if v[1].is_empty() && v[2].is_empty() {
        return Err(usage(
            "import requires --maildir <path> and/or --imapsync from=<host>",
        ));
    }
    let [address, maildir, host, user, port] = v;
    Ok(Plan::Run(
        run_env("import", json)
            .with("M_ADDRESS", address)
            .with("M_MAILDIR", maildir)
            .with("M_IMAPSYNC_HOST", host)
            .with("M_IMAPSYNC_USER", user)
            .with("M_IMAPSYNC_PASS", pass)
            .with("M_IMAPSYNC_PORT", port)
            .with("M_SKIP_INBOX", if skip { "true" } else { "false" }),
    ))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "server_enable" => enable_py(py_),
        "import" => import_py(py_),
        "server_disable" => {
            let d = py_.post("/cli/mail/server/disable", &obj(vec![]), line::DISABLE_CALL);
            d.and_then(|d| hint_or(py_, &d, "mail server disabled.", line::DISABLE_PRINT))
        }
        "server_uninstall" => uninstall_py(py_),
        "server_upgrade" => upgrade_py(py_),
        "rotate_admin" => {
            let d = py_.post(
                "/cli/mail/server/rotate-admin",
                &obj(vec![]),
                line::ROTATE_ADMIN_CALL,
            );
            d.and_then(|d| {
                hint_or(
                    py_,
                    &d,
                    "rotated leftover recovery principal and provisioned admin password — store kept",
                    line::ROTATE_ADMIN_CALL + 4,
                )
            })
        }
        "password_rotate" => password_rotate_py(py_),
        "app_password_list" => apw_list_py(py_),
        "app_password_add" => apw_add_py(py_),
        "app_password_revoke" => apw_revoke_py(py_),
        "credentials_keep" => keep_py(py_),
        "person_set" if py_.env("M_ALL") == "true" => person_all_py(py_),
        "person_set" => person_py(py_),
        "credentials_keep_all_mailbox" => keep_all_py(py_, true),
        "credentials_keep_all_app_password" => keep_all_py(py_, false),
        "person_link" => person_link_py(py_),
        _ => return None,
    })
}

fn enable_py(py_: &mut Py) -> R<()> {
    let hostname = py_.env("M_HOSTNAME").to_string();
    if hostname.is_empty() {
        return Err(py_.die_with(
            "usage",
            &format!("{CLI_NAME} hostmail enable --hostname <name> (e.g. mail.acme.dev)"),
            Some(Exit::Usage),
        ));
    }
    let d = py_.post(
        "/cli/mail/server/enable",
        &obj(vec![("hostname", sv(&hostname))]),
        line::ENABLE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
    } else if p::truthy(g(&d, "alreadyEnabled", line::ENABLE_ALREADY)?.as_ref()) {
        let h = or_s(
            g(&d, "hint", line::ENABLE_ALREADY + 1)?,
            "already enabled (no-op)",
        );
        py_.print(&h);
    } else {
        if let Some(h) = g(&d, "helper", line::ENABLE_HELPER)?.filter(py::truthy) {
            py_.print(&format!("helper   : {}", py::str_of(&h)));
        }
        let h = or_s(
            g(&d, "hint", line::ENABLE_HINT)?,
            "mail server: enable requested.",
        );
        py_.print(&h);
    }
    Ok(())
}

fn import_py(py_: &mut Py) -> R<()> {
    let mut body = vec![("address", pyx::env_v(py_, "M_ADDRESS"))];
    if !py_.env("M_MAILDIR").is_empty() {
        body.push(("maildir", pyx::env_v(py_, "M_MAILDIR")));
    }
    if py_.env("M_SKIP_INBOX") == "true" {
        body.push(("skipInbox", Value::Bool(true)));
    }
    if !py_.env("M_IMAPSYNC_HOST").is_empty() {
        let mut im = vec![("host", pyx::env_v(py_, "M_IMAPSYNC_HOST"))];
        if !py_.env("M_IMAPSYNC_USER").is_empty() {
            im.push(("user", pyx::env_v(py_, "M_IMAPSYNC_USER")));
        }
        if !py_.env("M_IMAPSYNC_PASS").is_empty() {
            im.push(("password", pyx::env_v(py_, "M_IMAPSYNC_PASS")));
        }
        if !py_.env("M_IMAPSYNC_PORT").is_empty() {
            let n = pyx::int(&pyx::env_v(py_, "M_IMAPSYNC_PORT"), line::IMPORT_PORT)?;
            im.push(("port", Value::Int(n)));
        }
        body.push(("imapsync", obj(im)));
    }
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/import",
        &[],
        Some(&obj(body)),
        Secs::Int(7200),
        None,
        line::IMPORT_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::IMPORT_PRINT;
    let text = match g(&d, "hint", l)?.filter(py::truthy) {
        Some(h) => py::str_of(&h),
        None => {
            let n = p::get_or(&d, "imported", sv("?")).at(l, MODULE)?;
            let fallback = if py_.env("M_ADDRESS").is_empty() {
                sv("?")
            } else {
                pyx::env_v(py_, "M_ADDRESS")
            };
            let a = p::get_or(&d, "address", fallback).at(l, MODULE)?;
            format!(
                "imported {} message(s) into {}",
                py::str_of(&n),
                py::str_of(&a)
            )
        }
    };
    py_.print(&text);
    Ok(())
}

fn uninstall_py(py_: &mut Py) -> R<()> {
    let hostname = py_.env("M_HOSTNAME").to_string();
    if hostname.is_empty() {
        return Err(py_.die_with(
            "usage",
            &format!("{CLI_NAME} hostmail uninstall --purge --confirm-hostname <host>"),
            Some(Exit::Usage),
        ));
    }
    let d = py_.post(
        "/cli/mail/server/uninstall",
        &obj(vec![
            ("purgeData", Value::Bool(true)),
            ("confirmHostname", sv(&hostname)),
        ]),
        line::UNINSTALL_CALL,
    )?;
    hint_or(py_, &d, "mail server uninstalled.", line::UNINSTALL_PRINT)
}

/// `float(v)`, or the exception's text.
fn to_float(v: &Value) -> Result<f64, String> {
    match v {
        Value::Int(d) => d
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .ok_or_else(|| "OverflowError: int too large to convert to float".into()),
        Value::Float(f) => Ok(*f),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Str(t) => py_float(&p::text(t)).ok_or_else(|| {
            format!(
                "ValueError: could not convert string to float: {}",
                py::repr(v)
            )
        }),
        other => Err(format!(
            "TypeError: float() argument must be a string or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// `"%.1f" % f`.
fn f1(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        format!("{f:.1}")
    }
}

/// `_hb(n)` (a byte count for people), called from line `from`.
fn hb(n: Option<Value>, from: u32) -> R<String> {
    let inside = |l: u32, e: String| {
        End::Raise(Tb {
            frames: vec![(from, MODULE), (l, "_hb")],
            exception: e,
        })
    };
    let n = match n {
        None | Some(Value::Null) => return Ok("?".into()),
        Some(v) => v,
    };
    let mut f = to_float(&n).map_err(|e| inside(line::HB_FLOAT, e))?;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if f < 1024.0 || unit == "TiB" {
            return if unit == "B" {
                let d = py::percent_d(&Value::Float(f)).map_err(|e| inside(line::HB_FORMAT, e))?;
                Ok(format!("{d} B"))
            } else {
                Ok(format!("{} {unit}", f1(f)))
            };
        }
        f /= 1024.0;
    }
    unreachable!("TiB returns")
}

fn upgrade_py(py_: &mut Py) -> R<()> {
    if py_.env("M_ACK_FAILED") == "true" {
        let d = py_.post(
            "/cli/mail/server/upgrade",
            &obj(vec![("acknowledgeFailed", Value::Bool(true))]),
            line::ACK_CALL,
        )?;
        return hint_or(py_, &d, "acknowledged.", line::ACK_PRINT);
    }
    let post = |py_: &Py, body: Value, l: u32| {
        py_.call_full(
            Method::Post,
            "/cli/mail/server/upgrade",
            &[],
            Some(&body),
            Secs::Int(120),
            None,
            l,
        )
    };
    if py_.env("M_DRY_RUN") == "true" {
        let d = post(
            py_,
            obj(vec![("dryRun", Value::Bool(true))]),
            line::DRY_CALL,
        )?;
        dry_run_print(py_, &d)?;
        let l = line::DRY_NOTES + 2;
        if p::truthy(g(&d, "dryRun", l)?.as_ref()) && !p::truthy(g(&d, "ready", l)?.as_ref()) {
            return Err(End::Exit(Exit::Error));
        }
        return Ok(());
    }
    let d = post(py_, obj(vec![]), line::UP_CALL)?;
    let l = line::UP_CALL + 1;
    if g(&d, "outcome", l)?.is_some_and(|v| p::eq_str(&v, "noop")) {
        hint_or(
            py_,
            &d,
            "already on the pinned version (no-op)",
            line::UP_CALL + 2,
        )?;
        return Err(End::Exit(Exit::Ok));
    }
    if !py_.json {
        let l = line::UP_OUTAGE;
        let out = or_dict(g(&d, "outage", l - 3)?);
        let from = or_s(g(&d, "from", l - 1)?, "?");
        let to = or_s(g(&d, "to", l - 1)?, "?");
        let typical = p::get_or(&out, "typicalSecs", sv("?")).at(l, MODULE)?;
        py_.print(&format!(
            "upgrading Stalwart {from} -> {to} in the background (outage ~{}s; Ctrl-C stops watching, not the upgrade)",
            py::str_of(&typical)
        ));
    }
    let setting = |names: &[&'static str], default: f64| -> f64 {
        env_names::first_nonempty_os_in(names, |n| std::env::var_os(n))
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .map_or(Some(default), |t| py_float(&t))
            .unwrap_or(default)
    };
    let interval = setting(env_names::HOSTMAIL_UPGRADE_POLL, 2.0);
    let lost_after = setting(env_names::HOSTMAIL_UPGRADE_LOST_SECS, 120.0);
    let deadline = Instant::now() + Duration::from_secs(6 * 3600);
    let mut seen = 0usize;
    let mut fin: Option<Value> = None;
    const TERMINAL: &[&str] = &[
        "succeeded",
        "aborted",
        "rolled_back",
        "rollback_failed",
        "interrupted",
    ];
    let mut last_ok = Instant::now();
    while Instant::now() < deadline {
        sleep(interval)?;
        let s = py_
            .request(Method::Get, "/cli/mail/status", &[], None, Secs::Int(30))
            .within(line::UP_SLEEP + 1, MODULE)?;
        let bad = match &s {
            Some(v @ Value::Object(_)) => p::truthy(g(v, "error", line::UP_SLEEP + 2)?.as_ref()),
            _ => true,
        };
        if bad {
            if last_ok.elapsed().as_secs_f64() >= lost_after {
                let secs =
                    py::percent_d(&Value::Float(lost_after)).at(line::UP_LOST + 7, MODULE)?;
                return Err(py_.die_with(
                    "daemon_lost",
                    &format!(
                        concat!(
                            "no upgrade status from the ",
                            product_name!(),
                            " daemon for {}s (it restarted, or is down). A restarted daemon finishes or rolls back the upgrade at start and starts Stalwart again: run `{} hostmail status --json` (upgrade.state, upgrade.recovery). Only if upgrade.state is still 'interrupted', follow upgrade.manualSteps, then `{} hostmail upgrade --acknowledge-failed`"
                        ),
                        secs, CLI_NAME, CLI_NAME
                    ),
                    None,
                ));
            }
            continue;
        }
        let s = s.unwrap_or(Value::Null);
        last_ok = Instant::now();
        let up = or_dict(g(&s, "upgrade", line::UP_UPGRADE)?);
        let steps = p::or(g(&up, "steps", line::UP_STEPS)?, Value::Array(Vec::new()));
        if !py_.json {
            for st in slice_from(&steps, seen, line::UP_STEP_FOR)? {
                let l = line::UP_STEP;
                let step = s_of(g(&st, "step", l)?);
                let ok = if p::truthy(g(&st, "ok", l)?.as_ref()) {
                    "ok"
                } else {
                    "FAILED"
                };
                let detail = or_s(g(&st, "detail", l + 1)?, "");
                py_.print(&format!(
                    "  {} {} {detail}",
                    pyx::ljust(&step, 18),
                    pyx::ljust(ok, 6)
                ));
            }
        }
        seen = pyx::len(&steps, line::UP_SEEN)?;
        let state = g(&up, "state", line::UP_SEEN + 1)?;
        if state.is_some_and(|v| TERMINAL.iter().any(|t| p::eq_str(&v, t))) {
            fin = Some(up);
            break;
        }
    }
    let Some(fin) = fin else {
        return Err(py_.die_with(
            "timeout",
            &format!("the upgrade is still running after 6h — check {CLI_NAME} hostmail status"),
            Some(Exit::Usage),
        ));
    };
    let l = line::UP_MANUAL - 12;
    let state = g(&fin, "state", l)?;
    if py_.json {
        py_.print_json(&fin);
    } else {
        let succeeded = state.as_ref().is_some_and(|v| p::eq_str(v, "succeeded"));
        if succeeded {
            let to = s_of(g(&fin, "to", l + 3)?);
            let snap = or_s(g(&fin, "snapshotDir", l + 3)?, "?");
            py_.print(&format!(
                "upgraded : Stalwart {to} (snapshot kept at {snap})"
            ));
        } else {
            py_.print(&format!("result   : {}", s_of(state.clone())));
        }
        if let Some(e) = g(&fin, "error", l + 6)?.filter(py::truthy) {
            py_.print(&format!("error    : {}", py::str_of(&e)));
        }
        if let Some(w) = g(&fin, "warning", l + 8)?.filter(py::truthy) {
            py_.print(&format!("WARNING  : {}", py::str_of(&w)));
        }
        if state
            .as_ref()
            .is_some_and(|v| p::eq_str(v, "rollback_failed"))
        {
            let mut err =
                String::from("ROLLBACK FAILED — mail may be down. Manual steps (as root):\n");
            let steps = items(g(&fin, "manualSteps", line::UP_MANUAL)?, line::UP_MANUAL);
            // What was written before a raise stays written.
            let (steps, raised) = match steps {
                Ok(s) => (s, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            for s in steps {
                err.push_str(&format!("  {}\n", py::str_of(&s)));
            }
            let _ = out::write_flushed(&mut std::io::stderr().lock(), err.as_bytes());
            if let Some(e) = raised {
                return Err(e);
            }
        }
    }
    let ok = g(&fin, "state", line::UP_MANUAL + 2)?.is_some_and(|v| p::eq_str(&v, "succeeded"));
    if !ok {
        return Err(End::Exit(Exit::Error));
    }
    Ok(())
}

/// `time.sleep(secs)`, or its exception at the loop's line.
fn sleep(secs: f64) -> R<()> {
    if secs.is_nan() {
        return Err(raise_at(
            line::UP_SLEEP,
            "ValueError: Invalid value NaN (not a number)",
        ));
    }
    if secs < 0.0 {
        return Err(raise_at(
            line::UP_SLEEP,
            "ValueError: sleep length must be non-negative",
        ));
    }
    if secs.is_infinite() || secs > 9.2e9 {
        return Err(raise_at(
            line::UP_SLEEP,
            "OverflowError: timestamp too large to convert to C _PyTime_t",
        ));
    }
    std::thread::sleep(Duration::from_secs_f64(secs));
    Ok(())
}

/// `v[seen:]` iterated, raised at `l`.
fn slice_from(v: &Value, seen: usize, l: u32) -> R<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.iter().skip(seen).cloned().collect()),
        Value::Str(_) => Ok(p::iter(v).at(l, MODULE)?.into_iter().skip(seen).collect()),
        Value::Object(_) => Err(raise_at(l, format!("KeyError: slice({seen}, None, None)"))),
        other => Err(raise_at(
            l,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

fn dry_run_print(py_: &mut Py, d: &Value) -> R<()> {
    if py_.json {
        py_.print_json(d);
        return Ok(());
    }
    let l = line::DRY_CALL + 3;
    if g(d, "outcome", l)?.is_some_and(|v| p::eq_str(&v, "noop")) {
        let h = or_s(
            g(d, "hint", l + 1)?,
            "already on the pinned version (no-op)",
        );
        py_.print(&h);
        return Ok(());
    }
    let ready = if p::truthy(g(d, "ready", l + 5)?.as_ref()) {
        "ready"
    } else {
        "BLOCKED — nothing will run"
    };
    py_.print(&format!(
        "upgrade  : Stalwart {} -> {}  ({ready})",
        or_s(g(d, "from", l + 4)?, "?"),
        or_s(g(d, "to", l + 4)?, "?")
    ));
    let state = or_s(g(d, "state", l + 7)?, "?");
    let was = if p::truthy(g(d, "wasRunning", l + 7)?.as_ref()) {
        "yes"
    } else {
        "no"
    };
    py_.print(&format!("server   : {state} (running now: {was})"));
    for b in items(g(d, "blockers", line::DRY_BLOCKERS)?, line::DRY_BLOCKERS)? {
        let code = s_of(g(&b, "code", line::DRY_BLOCKER)?);
        let hint = s_of(g(&b, "hint", line::DRY_BLOCKER)?);
        py_.print(&format!("blocked  : [{code}] {hint}"));
    }
    if !matches!(
        g(d, "dataBytes", line::DRY_HB - 2)?,
        None | Some(Value::Null)
    ) {
        let a = hb(g(d, "dataBytes", line::DRY_HB)?, line::DRY_HB)?;
        let b = hb(g(d, "configBytes", line::DRY_HB)?, line::DRY_HB)?;
        let snap = or_s(g(d, "snapshotDir", line::DRY_HB_FREE)?, "?");
        let free = hb(g(d, "freeBytes", line::DRY_HB_FREE)?, line::DRY_HB_FREE)?;
        py_.print(&format!(
            "data     : {a} data + {b} config -> snapshot {snap} (free {free})"
        ));
    }
    if let Some(out @ Value::Object(_)) = g(d, "outage", line::DRY_OUTAGE - 3)? {
        let t = s_of(g(&out, "typicalSecs", line::DRY_OUTAGE)?);
        let w = s_of(g(&out, "worstCaseSecs", line::DRY_OUTAGE)?);
        py_.print(&format!(
            "outage   : ~{t}s typical; up to {w}s if it fails and rolls back"
        ));
        let a = s_of(g(&out, "assumption", line::DRY_ASSUMPTION)?);
        py_.print(&format!("           ({a})"));
    }
    py_.print("plan     :");
    for (i, s) in items(g(d, "plan", line::DRY_PLAN)?, line::DRY_PLAN)?
        .into_iter()
        .enumerate()
    {
        py_.print(&format!("  {}. {}", i + 1, py::str_of(&s)));
    }
    py_.print("rollback :");
    for s in items(g(d, "rollback", line::DRY_ROLLBACK)?, line::DRY_ROLLBACK)? {
        py_.print(&format!("  - {}", py::str_of(&s)));
    }
    for n in items(g(d, "notes", line::DRY_NOTES)?, line::DRY_NOTES)? {
        py_.print(&format!("note     : {}", py::str_of(&n)));
    }
    Ok(())
}

fn password_rotate_py(py_: &mut Py) -> R<()> {
    let d = py_.post(
        "/cli/mail/address/password",
        &obj(vec![("address", pyx::env_v(py_, "M_ADDRESS"))]),
        line::PW_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::PW_CALL + 4;
    py_.print(&format!("rotated {}", addr(py_, &d, l)?));
    if let Some(pw) = g(&d, "password", l + 1)?.filter(py::truthy) {
        py_.print(&format!(
            "password : {}  (shown once — save it for Mail.app / IMAP)",
            py::str_of(&pw)
        ));
    }
    let user = match g(&d, "username", l + 3)?.filter(py::truthy) {
        Some(u) => py::str_of(&u),
        None => addr(py_, &d, l + 3)?,
    };
    py_.print(&format!("username : {user}"));
    for (label, key, port, from) in [
        ("imap    ", "imap", 443, line::PW_EP_CALL),
        ("submit  ", "submission", 465, line::PW_EP_SUBMIT),
        ("jmap    ", "jmap", 443, line::PW_EP_JMAP),
    ] {
        let ep_line = |e: String, l: u32| {
            End::Raise(Tb {
                frames: vec![(from, MODULE), (l, "_ep")],
                exception: e,
            })
        };
        let h = line::PW_EP_HOST;
        let ep = or_dict(p::attr_get(&d, key).map_err(|e| ep_line(e, h - 1))?);
        let host = or_s(p::attr_get(&ep, "host").map_err(|e| ep_line(e, h))?, "?");
        let port_v = p::attr_get(&ep, "port").map_err(|e| ep_line(e, h + 1))?;
        let port = or_s(port_v, &port.to_string());
        let alpn = p::attr_get(&ep, "alpn").map_err(|e| ep_line(e, h + 2))?;
        let extra = if p::truthy(alpn.as_ref()) {
            "  alpn"
        } else {
            ""
        };
        py_.print(&format!("{label} : {host}:{port}{extra}"));
    }
    if let Some(n) = g(&d, "apiKeysRevoked", line::PW_EP_JMAP + 1)? {
        py_.print(&format!("api keys : {} revoked", py::str_of(&n)));
    }
    if let Some(w) = g(&d, "apiKeysWarning", line::PW_EP_JMAP + 3)?.filter(py::truthy) {
        py_.print(&format!("WARNING  : {}", py::str_of(&w)));
    }
    py_.print(
        "note: this invalidates the mint-time secret — Mail.app sessions must use the new password",
    );
    py_.print(&format!(
        "note: the mailbox's API keys and app passwords stop working too ({CLI_NAME} hostmail app-password to add new ones)"
    ));
    Ok(())
}

fn apw_list_py(py_: &mut Py) -> R<()> {
    let params = vec![("address".to_string(), py_.env("M_ADDRESS").to_string())];
    let d = py_.get("/cli/mail/app-password", &params, line::APW_LIST_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(
        g(&d, "appPasswords", line::APW_LIST_LEN - 4)?,
        Value::Array(Vec::new()),
    );
    if !py::truthy(&rows) {
        let a = addr(py_, &d, line::APW_LIST_LEN - 2)?;
        py_.print(&format!("no app passwords for {a}"));
        return Ok(());
    }
    let n = pyx::len(&rows, line::APW_LIST_LEN)?;
    let a = addr(py_, &d, line::APW_LIST_LEN)?;
    py_.print(&format!("{n} app password(s) for {a}:"));
    for r in p::iter(&rows).at(line::APW_LIST_FOR, MODULE)? {
        let l = line::APW_LIST_ROW;
        let id = or_s(g(&r, "id", l)?, "?");
        let label = or_s(g(&r, "label", l)?, "");
        let at = or_s(g(&r, "createdAt", l)?, "");
        py_.print(&format!("  {id}  {label}  {at}"));
    }
    Ok(())
}

fn apw_add_py(py_: &mut Py) -> R<()> {
    let mut body = vec![("address", pyx::env_v(py_, "M_ADDRESS"))];
    if !py_.env("M_LABEL").is_empty() {
        body.push(("label", pyx::env_v(py_, "M_LABEL")));
    }
    let d = py_.post("/cli/mail/app-password", &obj(body), line::APW_ADD_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::APW_ADD_CALL + 4;
    py_.print(&format!("created app password for {}", addr(py_, &d, l)?));
    py_.print(&format!("id     : {}", or_s(g(&d, "id", l + 1)?, "?")));
    py_.print(&format!(
        "label  : {}",
        or_s(g(&d, "label", l + 2)?, CLI_NAME)
    ));
    if let Some(s) = g(&d, "secret", l + 3)?.filter(py::truthy) {
        py_.print(&format!(
            "secret : {}  (shown once — save it for Mail.app / IMAP)",
            py::str_of(&s)
        ));
    }
    Ok(())
}

fn apw_revoke_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        ("id", pyx::env_v(py_, "M_ID")),
    ]);
    let d = py_.post(
        "/cli/mail/app-password/revoke",
        &body,
        line::APW_REVOKE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::APW_REVOKE_CALL + 4;
    let id = pyx::or_env(py_, &d, "id", "M_ID", l)?;
    let a = addr(py_, &d, l)?;
    py_.print(&format!("revoked {id} on {a}"));
    Ok(())
}

fn keep_py(py_: &mut Py) -> R<()> {
    let mut body = vec![("address", pyx::env_v(py_, "M_ADDRESS"))];
    if !py_.env("M_ID").is_empty() {
        body.push(("appPasswordId", pyx::env_v(py_, "M_ID")));
    }
    let d = py_.post("/cli/mail/credentials/keep", &obj(body), line::KEEP_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::KEEP_CALL + 3;
    if let Some(id) = g(&d, "appPasswordId", l)?.filter(py::truthy) {
        let a = addr(py_, &d, l + 2)?;
        py_.print(&format!(
            "kept app password {} on {a} — the doctor will stop flagging it",
            py::str_of(&id)
        ));
    } else {
        let a = addr(py_, &d, l + 5)?;
        py_.print(&format!(
            "kept the mailbox password of {a} — the doctor will stop flagging it"
        ));
    }
    Ok(())
}

fn person_all_py(py_: &mut Py) -> R<()> {
    let on = py_.env("M_PERSON") == "on";
    let body = obj(vec![
        ("all", Value::Bool(true)),
        ("workspace", pyx::env_v(py_, "M_WORKSPACE")),
        ("person", Value::Bool(on)),
    ]);
    let d = py_.post("/cli/mail/address/person", &body, line::PERSON_ALL_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let changed = p::or(
        g(&d, "changed", line::PERSON_ALL_LEN - 2)?,
        Value::Array(Vec::new()),
    );
    let word = if on { "people's" } else { "agents'" };
    let n = pyx::len(&changed, line::PERSON_ALL_LEN)?;
    py_.print(&format!(
        "marked {n} mailbox(es) in {} as {word} mailboxes",
        py_.env("M_WORKSPACE")
    ));
    for a in p::iter(&changed).at(line::PERSON_ALL_FOR, MODULE)? {
        py_.print(&format!("  {}", py::str_of(&a)));
    }
    let excluded = g(&d, "excludedSendIdentities", line::PERSON_ALL_JOIN - 2)?;
    if let Some(ex) = excluded.filter(py::truthy) {
        let j = pyx::join(", ", &ex, line::PERSON_ALL_JOIN)?;
        py_.print(&format!(
            "left as the agent's own (its send identities): {j}"
        ));
    }
    Ok(())
}

fn person_py(py_: &mut Py) -> R<()> {
    let on = py_.env("M_PERSON") == "on";
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        ("person", Value::Bool(on)),
    ]);
    let d = py_.post("/cli/mail/address/person", &body, line::PERSON_CALL)?;
    if py_.json {
        py_.print_json(&d);
    } else if on {
        let a = addr(py_, &d, line::PERSON_CALL + 6)?;
        py_.print(&format!(
            "{a} is now a person's mailbox — the owner and IT agents can set its passwords"
        ));
    } else {
        let a = addr(py_, &d, line::PERSON_CALL + 8)?;
        py_.print(&format!("{a} is now an agent's mailbox"));
    }
    Ok(())
}

fn keep_all_py(py_: &mut Py, mailbox: bool) -> R<()> {
    let kind = if mailbox { "mailbox" } else { "app_password" };
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/credentials/keep",
        &[],
        Some(&obj(vec![
            ("allExisting", Value::Bool(true)),
            ("kind", sv(kind)),
        ])),
        Secs::Int(120),
        None,
        line::KEEP_ALL_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let kept = or_dict(g(&d, "kept", line::KEEP_ALL_KEPT - 2)?);
    let left = or_dict(g(&d, "leftFlagged", line::KEEP_ALL_KEPT - 1)?);
    let n = g(
        &kept,
        if mailbox {
            "mailboxPasswords"
        } else {
            "appPasswords"
        },
        line::KEEP_ALL_KEPT,
    )?;
    let what = if mailbox {
        "mailbox password(s)"
    } else {
        "app password(s)"
    };
    let n = match n {
        None | Some(Value::Null) => "0".to_string(),
        Some(v) => py::str_of(&v),
    };
    py_.print(&format!(
        "kept {n} {what} that existed before this update — the doctor will stop flagging them"
    ));
    let num = |key: &str, l: u32| -> R<String> {
        let v = p::or(g(&left, key, l)?, Value::Int("0".into()));
        let v = if py::truthy(&v) {
            v
        } else {
            Value::Int("0".into())
        };
        pyx::int(&v, l)
    };
    let newer = num("createdAfterCutoff", line::KEEP_ALL_NEWER)?;
    let undated = num("noCreationTime", line::KEEP_ALL_UNDATED)?;
    if newer != "0" || undated != "0" {
        py_.print(&format!(
            "still flagged: {newer} created after the update, {undated} with no creation time — review those one by one ({CLI_NAME} hostmail doctor)"
        ));
    }
    for e in items(
        g(&d, "listErrors", line::KEEP_ALL_ERRORS)?,
        line::KEEP_ALL_ERRORS,
    )? {
        py_.print(&format!("could not list app passwords: {}", py::str_of(&e)));
    }
    Ok(())
}

fn person_link_py(py_: &mut Py) -> R<()> {
    let who = py_.env("M_PERSON").to_string();
    let person = if who.is_empty() {
        Value::Null
    } else {
        sv(&who)
    };
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        ("person", person),
    ]);
    let d = py_.post(
        "/cli/mail/address/person-link",
        &body,
        line::PERSON_LINK_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
    } else if let Some(pv) = g(&d, "person", line::PERSON_LINK_CALL + 4)?.filter(py::truthy) {
        let a = addr(py_, &d, line::PERSON_LINK_CALL + 6)?;
        py_.print(&format!(
            "{a} is now linked to {}, who can manage its forwarding ({CLI_NAME} mail forward, or Settings)",
            py::str_of(&pv)
        ));
    } else {
        let a = addr(py_, &d, line::PERSON_LINK_CALL + 8)?;
        py_.print(&format!("{a} is no longer linked to a person"));
    }
    Ok(())
}
