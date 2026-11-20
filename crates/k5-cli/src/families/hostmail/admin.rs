//! The server's state and its domains, addresses and settings: `hostmail
//! status`, `domain` (add, list, show, check, remove), `create`,
//! `delete`, `doctor` and `config`, as the shell's `cmd_mail_*` and
//! `cmd_hostmail_delete` read the words (`cli/src/mail-watch.sh`,
//! `cli/src/mail-wrappers.sh`) and `_mail_py`'s verbs answer
//! (`cli/src/mail.sh`).
//!
//! `config --relay-pass-stdin` reads the password where the loop meets
//! the flag, as bash's `$(cat)` does; it goes out only in the request
//! body, and no child starts. A `--relay-port` too big for bash's `[`
//! is the shell's, decided before stdin is read.

use super::pyx::{self, g, items, or_s, s_of, sv};
use super::words::{scan, wants_help, Spec};
use crate::families::mail::core::{
    dumps, fits_bash, is_uint, obj, print, run_env, usage, At, End, Plan, Py, Secs, Tb, Within,
    MODULE, R,
};
use crate::families::mail::py::{self as p, big, py_index};
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::out::pylib::ops::py_eq;
use crate::transport::urllib::Method;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["status", "domain", "create", "delete", "doctor", "config"];

/// The daemon's name for itself as a certificate's owner: a wire value,
/// the same in every brand.
const WIRE_SELF: &str = "k2";

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const CREATE_CALL: u32 = 493;
    pub const CREATE_CAP: u32 = 498;
    pub const CREATE_EP_HOST: u32 = 512;
    pub const CREATE_EP_IMAP: u32 = 516;
    pub const CREATE_EP_SUBMIT: u32 = 517;
    pub const CREATE_EP_JMAP: u32 = 518;
    pub const DELETE_CALL: u32 = 849;
    pub const STATUS_REQUEST: u32 = 867;
    pub const STATUS_ERROR: u32 = 870;
    pub const STATUS_NAME0: u32 = 937;
    pub const STATUS_ORDER_JOIN: u32 = 949;
    pub const STATUS_ORDER_LEN: u32 = 950;
    pub const STATUS_PORTS: u32 = 954;
    pub const STATUS_PORT_STATES: u32 = 955;
    pub const STATUS_NAMES_LEN: u32 = 958;
    pub const STATUS_SERVES: u32 = 959;
    pub const STATUS_OWNER: u32 = 967;
    pub const STATUS_PENDING: u32 = 976;
    pub const STATUS_HERE: u32 = 989;
    pub const STATUS_BAD: u32 = 991;
    pub const STATUS_BAD_JOIN: u32 = 994;
    pub const DOMAIN_ADD_CALL: u32 = 1013;
    pub const DOMAIN_ADD_PRINT: u32 = 1017;
    pub const DOMAIN_CMDS_FOR: u32 = 1024;
    pub const DOMAIN_CMD: u32 = 1025;
    pub const DOMAIN_LIST_CALL: u32 = 1029;
    pub const DOMAIN_LIST_W: u32 = 1036;
    pub const DOMAIN_LIST_RECS: u32 = 1039;
    pub const DOMAIN_LIST_REC: u32 = 1042;
    pub const DOMAIN_LIST_NAG: u32 = 1045;
    pub const DOMAIN_LIST_ROW: u32 = 1047;
    pub const DOMAIN_SHOW_CALL: u32 = 1053;
    pub const DOMAIN_SHOW_PRINT: u32 = 1057;
    pub const DOMAIN_CHECK_CALL: u32 = 1060;
    pub const DOMAIN_CHECK_PRINT: u32 = 1064;
    pub const DOMAIN_REMOVE_REQUEST: u32 = 1069;
    pub const DOMAIN_REMOVE_ERROR: u32 = 1072;
    pub const DOMAIN_REMOVE_HINT: u32 = 1076;
    pub const DOCTOR_CALL: u32 = 1096;
    pub const CONFIG_GET_CALL: u32 = 1100;
    pub const FWD_CAP_CALL: u32 = 1104;
    pub const FWD_COPIES_CALL: u32 = 1114;
    pub const CONFIG_SET_INT: u32 = 1452;
    pub const CONFIG_SET_GB: u32 = 1454;
    pub const CONFIG_SET_CALL: u32 = 1464;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "status" => status(words),
        "domain" => domain(words),
        "create" => create(words),
        "delete" => delete(words),
        "doctor" => doctor(words),
        "config" => config(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// `cmd_mail_status`.
fn status(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/status"));
    }
    let bad =
        format!("unexpected argument '{{w}}' — usage: {CLI_NAME} mail status [--health] [--json]");
    let g = scan(
        words,
        &Spec {
            slots: &[],
            values: &[],
            switches: &[("--health", "health")],
            unknown: &bad,
            extra: &bad,
        },
    )?;
    Ok(Plan::Run(run_env("status", g.json).with(
        "M_HEALTH",
        if g.on("health") { "true" } else { "false" },
    )))
}

/// `cmd_mail_domain`.
fn domain(words: &[&str]) -> Result<Plan, Plan> {
    if words.is_empty() || wants_help(words) {
        return Ok(Plan::Help("hostmail/domain"));
    }
    let sub = words[0];
    let verb = match sub {
        "add" | "list" | "show" | "check" | "remove" => format!("domain_{sub}"),
        _ => {
            return Err(usage(format!(
                "unknown domain command '{sub}' — usage: {CLI_NAME} hostmail domain <add|list|show|check|remove>"
            )))
        }
    };
    let g = scan(
        &words[1..],
        &Spec {
            slots: &["domain"],
            values: &[],
            switches: &[("--yes", "yes"), ("--purge-data", "purge")],
            unknown: &format!("unknown flag '{{w}}' — see {CLI_NAME} hostmail domain --help"),
            extra: "unexpected extra argument '{w}' — one <domain> only",
        },
    )?;
    let d = g.get("domain");
    if sub != "remove" && (g.on("yes") || g.on("purge")) {
        return Err(usage(format!(
            "--yes/--purge-data only apply to '{CLI_NAME} hostmail domain remove'"
        )));
    }
    if sub != "list" && d.is_empty() {
        return Err(usage(format!("domain {sub} requires a <domain>")));
    }
    if sub == "list" && !d.is_empty() {
        return Err(usage(format!(
            "domain list takes no <domain> — did you mean '{CLI_NAME} hostmail domain show {d}'?"
        )));
    }
    let tf = |b: bool| if b { "true" } else { "false" };
    Ok(Plan::Run(
        run_env(&verb, g.json)
            .with("M_DOMAIN", d)
            .with("M_CONFIRM", tf(g.on("yes")))
            .with("M_PURGE", tf(g.on("purge"))),
    ))
}

/// `cmd_mail_create`.
fn create(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/create"));
    }
    let g = scan(
        words,
        &Spec {
            slots: &["local"],
            values: &[
                ("--domain", "domain", "--domain requires a value"),
                ("--id", "id", "--id requires a value (the idempotency key)"),
            ],
            switches: &[("--person", "person")],
            unknown: &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail create <localpart>[@<domain>] [--domain <d>] [--id <client-id>] [--person] [--json]"),
            extra: "unexpected extra argument '{w}' — one <localpart>[@<domain>] only",
        },
    )?;
    if g.get("local").is_empty() {
        return Err(usage(
            "create requires a <localpart> (e.g. research-bot or research-bot@acme.dev)",
        ));
    }
    Ok(Plan::Run(
        run_env("create", g.json)
            .with("M_LOCAL", g.get("local"))
            .with("M_DOMAIN", g.get("domain"))
            .with("M_CLIENT_ID", g.get("id"))
            .with("M_PERSON", if g.on("person") { "true" } else { "" }),
    ))
}

/// `cmd_hostmail_delete`.
fn delete(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/delete"));
    }
    let g = scan(
        words,
        &Spec {
            slots: &["address"],
            values: &[],
            switches: &[],
            unknown: &format!(
                "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail delete <address> [--json]"
            ),
            extra: "unexpected extra argument '{w}' — one <address> only",
        },
    )?;
    if g.get("address").is_empty() {
        return Err(usage(format!(
            "delete requires the full <address> to retire (see '{CLI_NAME} mail addresses')"
        )));
    }
    Ok(Plan::Run(
        run_env("address_delete", g.json).with("M_ADDRESS", g.get("address")),
    ))
}

/// `cmd_mail_doctor`.
fn doctor(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/doctor"));
    }
    let g = scan(
        words,
        &Spec {
            slots: &["domain"],
            values: &[],
            switches: &[],
            unknown: &format!(
                "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail doctor [<domain>] [--json]"
            ),
            extra: "unexpected extra argument '{w}' — one optional <domain> only",
        },
    )?;
    Ok(Plan::Run(
        run_env("doctor", g.json).with("M_DOMAIN", g.get("domain")),
    ))
}

/// `cmd_mail_config`'s value flags: (flag, setting, the line when the
/// value is missing, counts as a setting, a whole number checked at the
/// flag with this line).
const CONFIG_VALUES: &[(&str, &str, &str, bool, Option<&str>)] = &[
    (
        "--forward-copies-per-day",
        "copies_day",
        "--forward-copies-per-day requires a number",
        false,
        Some("--forward-copies-per-day must be a whole number, 1 to 24000 (default 500)"),
    ),
    (
        "--forward-copies-per-hour",
        "copies_hour",
        "--forward-copies-per-hour requires a number",
        false,
        Some("--forward-copies-per-hour must be a whole number, 1 to 1000 (default 100)"),
    ),
    (
        "--forward-new-per-day",
        "new_day",
        "--forward-new-per-day requires a number",
        false,
        Some("--forward-new-per-day must be a whole number, 0 to 1000 (default 20)"),
    ),
    (
        "--send-mode",
        "M_SEND_MODE",
        "--send-mode requires a value (direct | relay | receive-only)",
        true,
        None,
    ),
    (
        "--domain",
        "M_DOMAIN",
        "--domain requires a value",
        true,
        None,
    ),
    (
        "--relay-host",
        "M_RELAY_HOST",
        "--relay-host requires a value",
        true,
        None,
    ),
    (
        "--relay-port",
        "M_RELAY_PORT",
        "--relay-port requires a port number",
        true,
        None,
    ),
    (
        "--relay-user",
        "M_RELAY_USER",
        "--relay-user requires a value",
        true,
        None,
    ),
    (
        "--agent-send",
        "M_AGENT_SEND",
        "--agent-send requires a value (off | approval | on)",
        true,
        None,
    ),
    (
        "--address-cap",
        "M_ADDRESS_CAP",
        "--address-cap requires a number (0 = unlimited)",
        true,
        None,
    ),
    (
        "--quota-bytes",
        "M_QUOTA_BYTES",
        "--quota-bytes requires a number (0 = unlimited)",
        true,
        None,
    ),
    (
        "--quota-gb",
        "M_QUOTA_GB",
        "--quota-gb requires a number (0 = unlimited)",
        true,
        None,
    ),
    (
        "--quota-messages",
        "M_QUOTA_MESSAGES",
        "--quota-messages requires a number (0 = unlimited)",
        true,
        None,
    ),
    (
        "--workspace",
        "M_WORKSPACE",
        "--workspace requires a workspace (name | path | UUID)",
        true,
        None,
    ),
    (
        "--always-bcc",
        "M_ALWAYS_BCC",
        "--always-bcc requires an address list (comma-separated; '' clears)",
        true,
        None,
    ),
];

/// A `--relay-port` value bash's `[` can't read: its error lines and the
/// rest are the shell's.
fn relay_port_bash_cannot_read(words: &[&str]) -> bool {
    let mut i = 0;
    let mut last = None;
    while i < words.len() {
        let w = words[i];
        if CONFIG_VALUES.iter().any(|(f, ..)| *f == w) {
            if w == "--relay-port" {
                last = words.get(i + 1).copied();
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    last.is_some_and(|v| is_uint(v) && !fits_bash(v))
}

/// `cmd_mail_config`.
fn config(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/config"));
    }
    if relay_port_bash_cannot_read(words) {
        return Ok(Plan::Shell);
    }
    let mut vals: Vec<(&'static str, String)> = Vec::new();
    let get = |vals: &Vec<(&'static str, String)>, k: &str| -> String {
        vals.iter()
            .rev()
            .find(|(key, _)| *key == k)
            .map_or(String::new(), |(_, v)| v.clone())
    };
    let (mut json, mut any_set, mut bcc_set) = (false, false, false);
    let mut pass = String::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if let Some((_, key, missing, sets, uint)) = CONFIG_VALUES.iter().find(|(f, ..)| *f == w) {
            let v = words.get(i + 1).ok_or_else(|| usage(*missing))?;
            if let Some(bad) = uint {
                if !is_uint(v) {
                    return Err(usage(*bad));
                }
            }
            vals.push((key, v.to_string()));
            any_set |= sets;
            bcc_set |= *key == "M_ALWAYS_BCC";
            i += 2;
            continue;
        }
        match w {
            "--relay-pass-stdin" => {
                pass = super::stdin_all();
                any_set = true;
                if pass.is_empty() {
                    return Err(usage("--relay-pass-stdin: no password on stdin"));
                }
            }
            "--json" => json = true,
            other => {
                return Err(usage(format!(
                    "unknown argument '{other}' — see {CLI_NAME} hostmail config --help"
                )))
            }
        }
        i += 1;
    }
    let mode = get(&vals, "M_SEND_MODE");
    if !mode.is_empty() && !matches!(mode.as_str(), "direct" | "relay" | "receive-only") {
        return Err(usage(format!(
            "invalid --send-mode '{mode}' — valid: direct, relay, receive-only"
        )));
    }
    let agent = get(&vals, "M_AGENT_SEND");
    if !agent.is_empty() && !matches!(agent.as_str(), "off" | "approval" | "on") {
        return Err(usage(format!(
            "invalid --agent-send '{agent}' — valid: off, approval, on"
        )));
    }
    let port = get(&vals, "M_RELAY_PORT");
    if !port.is_empty()
        && (!is_uint(&port) || {
            let n = crate::families::mail::core::bash_num(&port);
            !(1..=65535).contains(&n)
        })
    {
        return Err(usage("--relay-port must be a port number (1-65535)"));
    }
    let cap = get(&vals, "M_ADDRESS_CAP");
    if !cap.is_empty() && !is_uint(&cap) {
        return Err(usage(
            "--address-cap must be a non-negative number (0 = unlimited)",
        ));
    }
    let (qb, qg, qm) = (
        get(&vals, "M_QUOTA_BYTES"),
        get(&vals, "M_QUOTA_GB"),
        get(&vals, "M_QUOTA_MESSAGES"),
    );
    if !qb.is_empty() && !qg.is_empty() {
        return Err(usage("give either --quota-bytes or --quota-gb, not both"));
    }
    for (v, flag) in [
        (&qb, "--quota-bytes"),
        (&qg, "--quota-gb"),
        (&qm, "--quota-messages"),
    ] {
        if !v.is_empty() && !is_uint(v) {
            return Err(usage(format!(
                "{flag} must be a non-negative number (0 = unlimited)"
            )));
        }
    }
    let ws = get(&vals, "M_WORKSPACE");
    if bcc_set && ws.is_empty() {
        return Err(usage(
            "--always-bcc needs --workspace <ws> — the policy is set per workspace",
        ));
    }
    let (day, hour, new) = (
        get(&vals, "copies_day"),
        get(&vals, "copies_hour"),
        get(&vals, "new_day"),
    );
    if !day.is_empty() || !hour.is_empty() {
        if any_set || !new.is_empty() || (!day.is_empty() && !hour.is_empty()) {
            return Err(usage("give --forward-copies-per-day or --forward-copies-per-hour on its own, one at a time, not with other settings"));
        }
        let (verb, n) = if day.is_empty() {
            ("config_forward_copies_hour", hour)
        } else {
            ("config_forward_copies_day", day)
        };
        return Ok(Plan::Run(run_env(verb, json).with("M_PER_DAY", n)));
    }
    if !new.is_empty() {
        if any_set {
            return Err(usage(
                "give --forward-new-per-day on its own, not with other settings",
            ));
        }
        return Ok(Plan::Run(
            run_env("config_forward_cap", json).with("M_PER_DAY", new),
        ));
    }
    let mut env = run_env(if any_set { "config_set" } else { "config_get" }, json);
    for key in [
        "M_SEND_MODE",
        "M_DOMAIN",
        "M_RELAY_HOST",
        "M_RELAY_PORT",
        "M_RELAY_USER",
        "M_AGENT_SEND",
        "M_ADDRESS_CAP",
        "M_QUOTA_BYTES",
        "M_QUOTA_GB",
        "M_QUOTA_MESSAGES",
        "M_WORKSPACE",
        "M_ALWAYS_BCC",
    ] {
        env.set(key, get(&vals, key));
    }
    env.set("M_RELAY_PASS", pass);
    env.set("M_ALWAYS_BCC_SET", if bcc_set { "true" } else { "false" });
    Ok(Plan::Run(env))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "create" => create_py(py_),
        "address_delete" => delete_py(py_),
        "status" => status_py(py_),
        "domain_add" => domain_add_py(py_),
        "domain_list" => domain_list_py(py_),
        "domain_show" => {
            let params = vec![("domain".to_string(), py_.env("M_DOMAIN").to_string())];
            py_.get("/cli/mail/domain/show", &params, line::DOMAIN_SHOW_CALL)
                .and_then(|d| print_domain(py_, &d, line::DOMAIN_SHOW_PRINT))
        }
        "domain_check" => {
            let body = obj(vec![("domain", pyx::env_v(py_, "M_DOMAIN"))]);
            py_.post("/cli/mail/domain/check", &body, line::DOMAIN_CHECK_CALL)
                .and_then(|d| print_domain(py_, &d, line::DOMAIN_CHECK_PRINT))
        }
        "domain_remove" => domain_remove_py(py_),
        "doctor" => {
            let mut body = vec![];
            if !py_.env("M_DOMAIN").is_empty() {
                body.push(("domain", pyx::env_v(py_, "M_DOMAIN")));
            }
            py_.post("/cli/mail/doctor", &obj(body), line::DOCTOR_CALL)
                .map(|d| py_.print(&dumps(&d)))
        }
        "config_get" => py_
            .get("/cli/mail/config", &[], line::CONFIG_GET_CALL)
            .map(|d| py_.print(&dumps(&d))),
        "config_forward_cap" => forward_cap_py(py_),
        "config_forward_copies_day" => forward_copies_py(py_, true),
        "config_forward_copies_hour" => forward_copies_py(py_, false),
        "config_set" => config_set_py(py_),
        _ => return None,
    })
}

/// `print_domain(d)` (or `print(dumps(d))`), from module line `l`.
fn print_domain(py_: &mut Py, d: &Value, l: u32) -> R<()> {
    if py_.json {
        py_.print_json(d);
        return Ok(());
    }
    print::domain(py_, d).within(l, MODULE)
}

/// `_ep(label, key, default_port)` of `create`, called from line `from`.
fn endpoint(py_: &mut Py, d: &Value, label: &str, key: &str, port: u32, from: u32) -> R<()> {
    let fr = |e: String, l: u32| {
        End::Raise(Tb {
            frames: vec![(from, MODULE), (l, "_ep")],
            exception: e,
        })
    };
    let h = line::CREATE_EP_HOST;
    let ep = pyx::or_dict(p::attr_get(d, key).map_err(|e| fr(e, h - 1))?);
    let host = or_s(p::attr_get(&ep, "host").map_err(|e| fr(e, h))?, "?");
    let port = or_s(
        p::attr_get(&ep, "port").map_err(|e| fr(e, h + 1))?,
        &port.to_string(),
    );
    let alpn = p::attr_get(&ep, "alpn").map_err(|e| fr(e, h + 2))?;
    let extra = if p::truthy(alpn.as_ref()) {
        "  alpn"
    } else {
        ""
    };
    py_.print(&format!("{label} : {host}:{port}{extra}"));
    Ok(())
}

fn create_py(py_: &mut Py) -> R<()> {
    let mut body = vec![
        ("project", sv(&py_.ws)),
        ("localPart", pyx::env_v(py_, "M_LOCAL")),
    ];
    if !py_.env("M_DOMAIN").is_empty() {
        body.push(("domain", pyx::env_v(py_, "M_DOMAIN")));
    }
    if !py_.env("M_CLIENT_ID").is_empty() {
        body.push(("clientId", pyx::env_v(py_, "M_CLIENT_ID")));
    }
    if py_.env("M_PERSON") == "true" {
        body.push(("person", Value::Bool(true)));
    }
    let d = py_.post("/cli/mail/address/create", &obj(body), line::CREATE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let existing = p::truthy(g(&d, "existing", line::CREATE_CAP - 1)?.as_ref());
    let word = if existing { "existing" } else { "created" };
    let l = line::CREATE_CAP;
    let a = py::str_of(&p::get_or(&d, "address", sv("?")).at(l, MODULE)?);
    let cap = print::cap_str(g(&d, "cap", l)?.as_ref()).within(l, MODULE)?;
    py_.print(&format!("{word} {a}   ({cap})"));
    if let Some(n) = g(&d, "note", l + 1)?.filter(py::truthy) {
        py_.print(&format!("note: {}", py::str_of(&n)));
    }
    if existing {
        py_.print("(idempotent --id match — this address was already minted with this id)");
        return Ok(());
    }
    if let Some(pw) = g(&d, "password", l + 6)?.filter(py::truthy) {
        py_.print(&format!(
            "password : {}  (shown once — save it for Mail.app / IMAP)",
            py::str_of(&pw)
        ));
    } else if p::truthy(g(&d, "passwordWithheld", l + 8)?.as_ref()) {
        let fallback = format!(
            "agents send with `{CLI_NAME} mail send`; only the owner can set an IMAP/SMTP password"
        );
        py_.print(&format!(
            "password : withheld — {}",
            or_s(g(&d, "passwordHint", l + 9)?, &fallback)
        ));
    }
    let user = match g(&d, "username", l + 11)?.filter(py::truthy) {
        Some(u) => py::str_of(&u),
        None => or_s(g(&d, "address", l + 11)?, "?"),
    };
    py_.print(&format!("username : {user}"));
    endpoint(py_, &d, "imap    ", "imap", 443, line::CREATE_EP_IMAP)?;
    endpoint(
        py_,
        &d,
        "submit  ",
        "submission",
        465,
        line::CREATE_EP_SUBMIT,
    )?;
    endpoint(py_, &d, "jmap    ", "jmap", 443, line::CREATE_EP_JMAP)
}

fn delete_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("project", sv(&py_.ws)),
        ("address", pyx::env_v(py_, "M_ADDRESS")),
    ]);
    let d = py_.post("/cli/mail/address/delete", &body, line::DELETE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = p::get_or(&d, "address", sv("?")).at(line::DELETE_CALL + 5, MODULE)?;
    py_.print(&format!(
        "retired {} — its cap slot is free immediately.",
        py::str_of(&a)
    ));
    py_.print("(the mailbox data is kept for the retention window, then purged)");
    Ok(())
}

/// `request(…)` and `die`'s two first checks shared by `status` and
/// `domain remove`: the answer, or the end.
fn requested(py_: &Py, d: Option<Value>, l_err: u32) -> R<Value> {
    let Some(d) = d else {
        return Err(py_.unreachable());
    };
    if let Some(e) = g(&d, "error", l_err)?.filter(py::truthy) {
        return Err(py_.die(&e, None, None));
    }
    Ok(d)
}

/// `die({"code": "error", "hint": "unexpected daemon response: …"})`.
fn unexpected(py_: &Py, d: &Value) -> End {
    let t: String = dumps(d).chars().take(200).collect();
    py_.die_with("error", &format!("unexpected daemon response: {t}"), None)
}

/// `"key" in d`.
fn has(d: &Value, key: &str) -> bool {
    matches!(d, Value::Object(pairs) if pairs.iter().any(|(k, _)| p::text(k) == key))
}

/// `d[key] = v`: in place, or at the end.
fn put(d: &mut Value, key: &str, v: Value) {
    if let Value::Object(pairs) = d {
        match pairs.iter_mut().find(|(k, _)| p::text(k) == key) {
            Some(slot) => slot.1 = v,
            None => pairs.push((Text::from(key), v)),
        }
    }
}

/// `v[0]`, raised at `l`.
fn first(v: &Value, l: u32) -> R<Value> {
    match v {
        Value::Array(a) => a.first().cloned().ok_or_else(|| {
            crate::families::mail::core::raise_at(l, "IndexError: list index out of range")
        }),
        Value::Str(_) => Ok(p::iter(v)
            .at(l, MODULE)?
            .into_iter()
            .next()
            .unwrap_or(Value::Null)),
        Value::Object(_) => Err(crate::families::mail::core::raise_at(l, "KeyError: 0")),
        other => Err(crate::families::mail::core::raise_at(
            l,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

fn on_off(v: Option<Value>, on: &'static str, off: &'static str) -> &'static str {
    if p::truthy(v.as_ref()) {
        on
    } else {
        off
    }
}

fn status_py(py_: &mut Py) -> R<()> {
    let mut params = vec![("fresh".to_string(), "1".to_string())];
    if py_.env("M_HEALTH") == "true" {
        params.push(("health".to_string(), "1".to_string()));
    }
    let d = py_
        .request(
            Method::Get,
            "/cli/mail/status",
            &params,
            None,
            Secs::Int(30),
        )
        .within(line::STATUS_REQUEST, MODULE)?;
    let mut d = requested(py_, d, line::STATUS_ERROR)?;
    if !p::truthy(g(&d, "ok", line::STATUS_ERROR + 2)?.as_ref()) && !has(&d, "state") {
        return Err(unexpected(py_, &d));
    }
    if !has(&d, "consistent") {
        let ok = p::truthy(g(&d, "ok", line::STATUS_ERROR + 5)?.as_ref());
        put(&mut d, "consistent", Value::Bool(ok));
        put(&mut d, "ok", Value::Bool(true));
    }
    let d = d;
    let supported = p::truthy(g(&d, "supported", 879)?.as_ref());
    let state = g(&d, "state", 879)?;
    if py_.json {
        py_.print_json(&d);
    } else if !supported
        || state
            .as_ref()
            .is_some_and(|v| p::eq_str(v, "not-installed"))
    {
        if !supported {
            py_.print(&format!("server   : not available here — hosted mail runs on Linux {PRODUCT_NAME} servers. For linked inboxes use `{CLI_NAME} mail inboxes`."));
        } else {
            py_.print(&format!("server   : not installed — hosted mail runs on Linux {PRODUCT_NAME} servers ({CLI_NAME} hostmail enable --hostname <mail host>). For linked inboxes use `{CLI_NAME} mail inboxes`."));
            if let Some(h) = g(&d, "helper", 887)?.filter(py::truthy) {
                py_.print(&format!("helper   : {}", py::str_of(&h)));
                if let Some(f) = g(&d, "helperFix", 889)?.filter(py::truthy) {
                    py_.print(&format!("fix      : {}", py::str_of(&f)));
                }
            }
        }
    } else {
        status_lines(py_, &d)?;
    }
    let consistent_false = matches!(g(&d, "consistent", 1009)?, Some(Value::Bool(false)));
    let running = g(&d, "state", 1009)?.is_some_and(|v| p::eq_str(&v, "running"));
    if consistent_false || !running {
        return Err(End::Exit(crate::exit::Exit::Error));
    }
    Ok(())
}

/// The installed server's lines of `status`.
fn status_lines(py_: &mut Py, d: &Value) -> R<()> {
    let state = py::str_of(&p::get_or(d, "state", sv("?")).at(893, MODULE)?);
    let yes = on_off(g(d, "supported", 893)?, "yes", "no");
    py_.print(&format!(
        "server   : {state}  (email supported on this daemon: {yes})"
    ));
    let sv_ = match g(d, "version", 895)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => or_s(g(d, "pinnedVersion", 895)?, "?"),
    };
    py_.print(&format!("license  : Stalwart {sv_} (AGPL-3.0-only), unmodified; source: https://github.com/Alakazam-211/stalwart/tree/v{sv_}"));
    if matches!(g(d, "consistent", 898)?, Some(Value::Bool(false))) {
        py_.print(&format!(
            "WARNING  : status and systemd disagree — {}",
            or_s(g(d, "lastError", 900)?, "no detail from the daemon")
        ));
        py_.print("           ground truth: systemctl is-active stalwart (read-only; do not start/stop it by hand)");
    }
    let version = or_s(g(d, "version", 904)?, "(not installed)");
    let pinned = py::str_of(&p::get_or(d, "pinnedVersion", sv("?")).at(904, MODULE)?);
    py_.print(&format!("version  : {version}  (pinned {pinned})"));
    if p::truthy(g(d, "upgradeAvailable", 905)?.as_ref()) {
        py_.print(&format!("upgrade  : available — {CLI_NAME} hostmail upgrade --dry-run, then {CLI_NAME} hostmail upgrade"));
    }
    if let Some(up @ Value::Object(_)) = g(d, "upgrade", 908)? {
        if let Some(st) = g(&up, "state", 909)?.filter(py::truthy) {
            let mut l = format!("last upgrade: {}", py::str_of(&st));
            if let Some(s) = g(&up, "step", 911)?.filter(py::truthy) {
                l.push_str(&format!(" (step {})", py::str_of(&s)));
            }
            if let Some(e) = g(&up, "error", 913)?.filter(py::truthy) {
                l.push_str(&format!(" — {}", py::str_of(&e)));
            }
            py_.print(&l);
            if p::eq_str(&st, "rollback_failed") || p::eq_str(&st, "interrupted") {
                py_.print(&format!("           manual steps: {CLI_NAME} hostmail status --json (upgrade.manualSteps), then {CLI_NAME} hostmail upgrade --acknowledge-failed"));
            }
        }
    }
    for (key, label, l) in [("hostname", "hostname", 919), ("portPlan", "plan    ", 921)] {
        if let Some(v) = g(d, key, l)?.filter(py::truthy) {
            py_.print(&format!("{label} : {}", py::str_of(&v)));
        }
    }
    if let Some(prog @ Value::Object(_)) = g(d, "enableProgress", 923)? {
        if let Some(c) = g(&prog, "current", 924)?.filter(py::truthy) {
            py_.print(&format!("install  : {}", py::str_of(&c)));
        }
    }
    if let Some(e) = g(d, "lastError", 926)?.filter(py::truthy) {
        py_.print(&format!("last err : {}", py::str_of(&e)));
    }
    if let Some(h) = g(d, "helper", 928)?.filter(py::truthy) {
        py_.print(&format!("helper   : {}", py::str_of(&h)));
        if let Some(f) = g(d, "helperFix", 932)?.filter(py::truthy) {
            py_.print(&format!("fix      : {}", py::str_of(&f)));
        }
    }
    if let Some(cert @ Value::Object(_)) = g(d, "cert", 934)? {
        status_cert(py_, &cert)?;
    }
    if let Some(ob @ Value::Object(_)) = g(d, "outbound", 968)? {
        let strat = g(&ob, "ipStrategy", 969)?;
        let dane = g(&ob, "daneSummary", 969)?;
        if p::truthy(strat.as_ref()) || p::truthy(dane.as_ref()) {
            let fallback = format!("not checked ({CLI_NAME} hostmail doctor)");
            py_.print(&format!(
                "outbound : {} · DANE {} · DNSSEC {}",
                or_s(strat, "?"),
                or_s(dane, "?"),
                or_s(g(&ob, "dnssec", 973)?, &fallback)
            ));
            if let Some(pr) = g(&ob, "pendingReload", 974)?.filter(py::truthy) {
                let e = or_s(g(&pr, "error", line::STATUS_PENDING)?, "?");
                py_.print(&format!(
                    "WARNING  : outbound settings saved but not live — {e}"
                ));
            }
        }
    }
    if let Some(ren @ Value::Object(_)) = g(d, "certRenewal", 977)? {
        if let Some(mh @ Value::Object(_)) = g(&ren, "mailHost", 978)? {
            if p::truthy(g(&mh, "k2Issued", 979)?.as_ref()) {
                let mut l = format!("renewal  : {}", or_s(g(&mh, "state", 981)?, "?"));
                if let Some(f) = g(&mh, "failures", 982)?.filter(py::truthy) {
                    l.push_str(&format!(
                        " — last attempt failed ({}): {}; next {}",
                        py::str_of(&f),
                        or_s(g(&mh, "lastError", 984)?, "?"),
                        or_s(g(&mh, "nextAttempt", 984)?, "?")
                    ));
                }
                py_.print(&l);
            }
        }
    }
    if let Some(Value::Array(extra)) = g(d, "extraNames", 986)? {
        if !extra.is_empty() {
            let mut here = Vec::new();
            for e in &extra {
                if matches!(
                    g(e, "pointsHere", line::STATUS_HERE)?,
                    Some(Value::Bool(true))
                ) {
                    here.push(e.clone());
                }
            }
            let mut bad = Vec::new();
            for e in &here {
                let c = pyx::or_dict(g(e, "cert", line::STATUS_BAD)?);
                let st = g(&c, "state", line::STATUS_BAD)?;
                if !st.is_some_and(|v| p::eq_str(&v, "issued")) {
                    bad.push(p::or(g(e, "name", line::STATUS_BAD - 1)?, sv("?")));
                }
            }
            let tail = if bad.is_empty() {
                String::new()
            } else {
                let j = pyx::join(", ", &Value::Array(bad), line::STATUS_BAD_JOIN)?;
                format!("; no own cert: {j}")
            };
            py_.print(&format!(
                "names    : {} of {} extra names point here{tail}",
                here.len(),
                extra.len()
            ));
        }
    }
    if let Some(cal @ Value::Object(_)) = g(d, "calendar", 995)? {
        let url = match g(&cal, "url", 1002)?.filter(py::truthy) {
            Some(u) => format!(" {}", py::str_of(&u)),
            None => String::new(),
        };
        py_.print(&format!(
            "calendar : {}, files {}, {}{url}",
            on_off(g(&cal, "enabled", 999)?, "on", "off"),
            on_off(g(&cal, "files", 1000)?, "on", "off"),
            on_off(g(&cal, "reachable", 1001)?, "reachable", "not reachable")
        ));
        if let Some(r) = g(&cal, "reason", 1003)?.filter(py::truthy) {
            py_.print(&format!("           {}", py::str_of(&r)));
        }
    }
    if let Some(h) = g(d, "health", 1005)? {
        if h != Value::Null {
            py_.print(&format!("health   : {}", dumps(&h)));
        }
    }
    Ok(())
}

/// `status`'s certificate lines.
fn status_cert(py_: &mut Py, cert: &Value) -> R<()> {
    let names = p::or(g(cert, "names", 936)?, Value::Array(vec![]));
    let host = match g(cert, "host", line::STATUS_NAME0)?.filter(py::truthy) {
        Some(h) => h,
        None if py::truthy(&names) => first(&names, line::STATUS_NAME0)?,
        None => sv(""),
    };
    let state = p::or(g(cert, "state", 938)?, sv("?"));
    let extra = on_off(g(cert, "selfSigned", 939)?, " self-signed", "");
    let exp = match g(cert, "expiresAt", 940)?.filter(py::truthy) {
        Some(e) => format!(" expires {}", py::str_of(&e)),
        None => String::new(),
    };
    let host_s = if py::truthy(&host) {
        p::concat(" ", &host).at(941, MODULE)?
    } else {
        String::new()
    };
    let mut l = format!("cert     : {}{host_s}{extra}{exp}", py::str_of(&state));
    let acme = match g(cert, "acme", 942)? {
        Some(a @ Value::Object(_)) => a,
        _ => Value::Object(vec![]),
    };
    let task = match g(&acme, "lastTask", 943)? {
        Some(t @ Value::Object(_)) => Some(t),
        _ => None,
    };
    if let Some(task) = task.filter(py::truthy) {
        let ts = g(&task, "state", 944)?;
        let retry = ts
            .as_ref()
            .is_some_and(|v| p::eq_str(v, "Retry") || p::eq_str(v, "Failed"));
        if retry && !p::eq_str(&state, "issued") {
            let failed = ts.as_ref().is_some_and(|v| p::eq_str(v, "Failed"));
            l.push_str(&format!(
                " — ACME {} {}: {}",
                if failed { "failed" } else { "retrying" },
                or_s(g(&task, "at", 946)?, "?"),
                or_s(g(&task, "reason", 946)?, "?")
            ));
        }
    }
    let order_names = g(&acme, "orderNames", 947)?;
    if !matches!(order_names, None | Some(Value::Null))
        && g(&acme, "mode", 947)?.is_some_and(|v| p::eq_str(&v, "Automatic"))
    {
        let order = p::or(g(&acme, "orderNames", 948)?, Value::Array(vec![]));
        let j = pyx::join(", ", &order, line::STATUS_ORDER_JOIN)?;
        let j = if j.is_empty() { "?".to_string() } else { j };
        let only = if pyx::len(&order, line::STATUS_ORDER_LEN)? == 1 {
            " only"
        } else {
            ""
        };
        l.push_str(&format!(" (orders: {j}{only})"));
    }
    py_.print(&l);
    let ports: Vec<Value> = items(g(cert, "ports", line::STATUS_PORTS)?, line::STATUS_PORTS)?
        .into_iter()
        .filter(|p| matches!(p, Value::Object(_)))
        .collect();
    let mut states: Vec<Value> = Vec::new();
    for port in &ports {
        let s = g(port, "state", line::STATUS_PORT_STATES)?.unwrap_or(Value::Null);
        if matches!(s, Value::Array(_) | Value::Object(_)) {
            return Err(crate::families::mail::core::raise_at(
                line::STATUS_PORT_STATES,
                format!("TypeError: unhashable type: '{}'", py::type_name(&s)),
            ));
        }
        if !states.iter().any(|x| py_eq(x, &s)) {
            states.push(s);
        }
    }
    if states.len() > 1 {
        let mut parts = Vec::new();
        for port in &ports {
            parts.push(format!(
                "{} {}",
                s_of(g(port, "port", 957)?),
                s_of(g(port, "state", 957)?)
            ));
        }
        py_.print(&format!("           ports: {}", parts.join(" · ")));
    }
    let n = pyx::len(&names, line::STATUS_NAMES_LEN)?;
    let differs = n > 1
        || (py::truthy(&names)
            && py::truthy(&host)
            && !py_eq(&first(&names, line::STATUS_NAMES_LEN)?, &host));
    if differs {
        let j = pyx::join(", ", &names, line::STATUS_SERVES)?;
        py_.print(&format!("           serves: {j}"));
    }
    if let Some(note) = g(cert, "note", 960)?.filter(py::truthy) {
        py_.print(&format!("           {}", py::str_of(&note)));
    }
    if let Some(owner) = g(cert, "owner", 962)?.filter(py::truthy) {
        let l = line::STATUS_OWNER;
        let why = match &owner {
            Value::Str(t) => match p::text(t).as_str() {
                WIRE_SELF => format!(
                    "{PRODUCT_NAME} issues, installs and renews it; Stalwart ACME is Manual"
                ),
                "stalwart-acme" => {
                    format!("Stalwart's own ACME; {PRODUCT_NAME} locks its names to the mail host")
                }
                "unknown" => format!(
                    "attached, but no {}-planted certificate yet ({CLI_NAME} hostmail cert renew)",
                    PRODUCT_NAME
                ),
                _ => "?".into(),
            },
            Value::Array(_) | Value::Object(_) => {
                return Err(crate::families::mail::core::raise_at(
                    l,
                    format!("TypeError: unhashable type: '{}'", py::type_name(&owner)),
                ))
            }
            _ => "?".into(),
        };
        py_.print(&format!("owner    : {} — {why}", py::str_of(&owner)));
    }
    Ok(())
}

fn domain_add_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("domain", pyx::env_v(py_, "M_DOMAIN"))]);
    let d = py_.post("/cli/mail/domain/add", &body, line::DOMAIN_ADD_CALL)?;
    print_domain(py_, &d, line::DOMAIN_ADD_PRINT)?;
    if py_.json {
        return Ok(());
    }
    let cmds = p::or(
        g(&d, "dnsCommands", line::DOMAIN_CMDS_FOR - 4)?,
        Value::Array(vec![]),
    );
    if py::truthy(&cmds) {
        py_.print("");
        py_.print(&format!(
            "This domain's zone is on {PRODUCT_NAME}. To publish the records:"
        ));
        for c in p::iter(&cmds).at(line::DOMAIN_CMDS_FOR, MODULE)? {
            let t = p::concat("  ", &c).at(line::DOMAIN_CMD, MODULE)?;
            py_.print(&t);
        }
    }
    Ok(())
}

/// `len(v)` inside `max(…)`'s generator, raised in its frame.
fn gen_len(r: &Value) -> R<usize> {
    let fr = |e: String| {
        End::Raise(Tb {
            frames: vec![
                (line::DOMAIN_LIST_W, MODULE),
                (line::DOMAIN_LIST_W, "<genexpr>"),
            ],
            exception: e,
        })
    };
    let v = p::get_or(r, "domain", sv("")).map_err(fr)?;
    p::len(&v).map_err(fr)
}

fn domain_list_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/domain/list", &[], line::DOMAIN_LIST_CALL)?;
    let rows = p::or(
        g(&d, "domains", line::DOMAIN_LIST_CALL + 1)?,
        Value::Array(vec![]),
    );
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    if !py::truthy(&rows) {
        py_.print(&format!(
            "No domains yet — add one: {CLI_NAME} hostmail domain add <domain>"
        ));
        return Ok(());
    }
    let rows = p::iter(&rows).at(line::DOMAIN_LIST_W, MODULE)?;
    let mut w = "DOMAIN".len();
    let mut widest = 0;
    for r in &rows {
        widest = widest.max(gen_len(r)?);
    }
    w = w.max(widest) + 2;
    let row = |a: &str, b: &str, c: &str, e: &str| {
        format!(
            "{}{}{}{e}",
            pyx::ljust(a, w),
            pyx::ljust(b, 11),
            pyx::ljust(c, 14)
        )
    };
    py_.print(&row("DOMAIN", "STATUS", "SEND-MODE", "RECORDS"));
    for r in &rows {
        let recs = pyx::or_dict(g(r, "records", line::DOMAIN_LIST_RECS)?);
        let mut parts = Vec::new();
        for k in ["valid", "missing", "wrong", "pending", "unknown"] {
            if p::truthy(g(&recs, k, line::DOMAIN_LIST_REC)?.as_ref()) {
                let v = py_index(&recs, k).at(line::DOMAIN_LIST_REC + 1, MODULE)?;
                parts.push(format!("{} {k}", py::str_of(&v)));
            }
        }
        let mut summary = if parts.is_empty() {
            "-".to_string()
        } else {
            parts.join(" · ")
        };
        if p::truthy(g(r, "dmarcNag", line::DOMAIN_LIST_NAG)?.as_ref()) {
            summary.push_str("  (DMARC recommended)");
        }
        let l = line::DOMAIN_LIST_ROW;
        let name = py::str_of(&p::get_or(r, "domain", sv("")).at(l, MODULE)?);
        let status = py::str_of(&p::get_or(r, "status", sv("?")).at(l + 1, MODULE)?);
        let mode = py::str_of(&p::get_or(r, "sendMode", sv("?")).at(l + 1, MODULE)?);
        py_.print(&row(&name, &status, &mode, &summary));
    }
    py_.print("");
    py_.print(&format!(
        "details: {CLI_NAME} hostmail domain show <domain>"
    ));
    Ok(())
}

fn domain_remove_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("domain", pyx::env_v(py_, "M_DOMAIN")),
        ("confirm", Value::Bool(py_.env("M_CONFIRM") == "true")),
        ("purge", Value::Bool(py_.env("M_PURGE") == "true")),
    ]);
    let d = py_
        .request(
            Method::Post,
            "/cli/mail/domain/remove",
            &[],
            Some(&body),
            Secs::Int(30),
        )
        .within(line::DOMAIN_REMOVE_REQUEST, MODULE)?;
    let Some(d) = d else {
        return Err(py_.unreachable());
    };
    if let Some(mut err) = g(&d, "error", line::DOMAIN_REMOVE_ERROR)?.filter(py::truthy) {
        if let Value::Object(_) = &err {
            if g(&err, "code", line::DOMAIN_REMOVE_HINT - 2)?
                .is_some_and(|v| p::eq_str(&v, "confirm_required"))
            {
                let h = p::get_or(&err, "hint", sv("")).at(line::DOMAIN_REMOVE_HINT, MODULE)?;
                let h = py::str_of(&h).replace("repeat with confirm: true", "re-run with --yes");
                put(&mut err, "hint", sv(&h));
            }
        }
        return Err(py_.die(&err, None, None));
    }
    if !p::truthy(g(&d, "ok", line::DOMAIN_REMOVE_ERROR + 7)?.as_ref()) {
        return Err(unexpected(py_, &d));
    }
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DOMAIN_REMOVE_ERROR + 13;
    let dom = py::str_of(&p::get_or(&d, "domain", sv("?")).at(l, MODULE)?);
    let n = py::str_of(&p::get_or(&d, "retiredAddresses", Value::Int("0".into())).at(l, MODULE)?);
    let del = on_off(g(&d, "stalwartDeleted", l + 1)?, "yes", "no");
    py_.print(&format!(
        "Removed {dom} (retired {n} address(es), mail-server domain deleted: {del})."
    ));
    if p::truthy(g(&d, "purge", l + 2)?.as_ref()) {
        py_.print("mail data purge requested.");
    }
    Ok(())
}

fn forward_cap_py(py_: &mut Py) -> R<()> {
    let n = pyx::int(&pyx::env_v(py_, "M_PER_DAY"), line::FWD_CAP_CALL)?;
    let d = py_.post(
        "/cli/mail/config/forward-new-per-day",
        &obj(vec![("perDay", Value::Int(n))]),
        line::FWD_CAP_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::FWD_CAP_CALL + 5;
    py_.print(&format!(
        "this server may now add {} new outside forward addresses a day (default {}, most {})",
        s_of(g(&d, "perDay", l)?),
        s_of(g(&d, "default", l)?),
        s_of(g(&d, "max", l)?)
    ));
    Ok(())
}

fn forward_copies_py(py_: &mut Py, daily: bool) -> R<()> {
    let field = if daily { "perDay" } else { "perHour" };
    let n = pyx::int(&pyx::env_v(py_, "M_PER_DAY"), line::FWD_COPIES_CALL)?;
    let route = if daily {
        "/cli/mail/config/forward-copies-per-day"
    } else {
        "/cli/mail/config/forward-copies-per-hour"
    };
    let d = py_.post(
        route,
        &obj(vec![(field, Value::Int(n))]),
        line::FWD_COPIES_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::FWD_COPIES_CALL + 5;
    py_.print(&format!(
        "each mailbox on this server may now forward {} copies {} (default {}, most {})",
        s_of(g(&d, field, l)?),
        if daily { "in 24 hours" } else { "an hour" },
        s_of(g(&d, "default", l)?),
        s_of(g(&d, "max", l)?)
    ));
    if let Some(n) = g(&d, "note", l + 1)?.filter(py::truthy) {
        py_.print(&format!("note: {}", py::str_of(&n)));
    }
    Ok(())
}

fn config_set_py(py_: &mut Py) -> R<()> {
    let mut body: Vec<(&str, Value)> = Vec::new();
    for (envk, key) in [
        ("M_SEND_MODE", "sendMode"),
        ("M_DOMAIN", "domain"),
        ("M_RELAY_HOST", "relayHost"),
        ("M_RELAY_PORT", "relayPort"),
        ("M_RELAY_USER", "relayUser"),
        ("M_RELAY_PASS", "relayPass"),
        ("M_AGENT_SEND", "agentSend"),
        ("M_ADDRESS_CAP", "addressCap"),
        ("M_QUOTA_BYTES", "quotaBytes"),
        ("M_QUOTA_MESSAGES", "quotaMessages"),
        ("M_WORKSPACE", "workspace"),
    ] {
        let v = py_.env(envk).to_string();
        if !v.is_empty() {
            let val = if matches!(
                key,
                "relayPort" | "addressCap" | "quotaBytes" | "quotaMessages"
            ) {
                Value::Int(pyx::int(&sv(&v), line::CONFIG_SET_INT)?)
            } else {
                sv(&v)
            };
            body.push((key, val));
        }
    }
    let set = |body: &mut Vec<(&str, Value)>, key: &'static str, v: Value| match body
        .iter_mut()
        .find(|(k, _)| *k == key)
    {
        Some(slot) => slot.1 = v,
        None => body.push((key, v)),
    };
    if !py_.env("M_QUOTA_GB").is_empty() {
        let n = pyx::int(&pyx::env_v(py_, "M_QUOTA_GB"), line::CONFIG_SET_GB)?;
        let bytes = if let Some(neg) = n.strip_prefix('-') {
            format!("-{}", big::mul_small(neg, 1 << 30))
        } else {
            big::mul_small(&n, 1 << 30)
        };
        set(&mut body, "quotaBytes", Value::Int(big::trim(&bytes)));
    }
    if py_.env("M_ALWAYS_BCC_SET") == "true" {
        let list: Vec<Value> = py_.env.list("M_ALWAYS_BCC").iter().map(|m| sv(m)).collect();
        set(&mut body, "alwaysBcc", Value::Array(list));
    }
    let quota_keys = ["quotaBytes", "quotaMessages"];
    let has_quota = body.iter().any(|(k, _)| quota_keys.contains(k));
    if has_quota && !body.iter().any(|(k, _)| *k == "workspace") {
        let mut defaults: Vec<(&str, Value)> = Vec::new();
        for k in quota_keys {
            if let Some(pos) = body.iter().position(|(key, _)| *key == k) {
                defaults.push(body.remove(pos));
            }
        }
        body.push(("defaults", obj(defaults)));
    }
    let d = py_.post("/cli/mail/config/set", &obj(body), line::CONFIG_SET_CALL)?;
    pyx::hint_or(py_, &d, "mail config updated.", line::CONFIG_SET_CALL + 4)
}
