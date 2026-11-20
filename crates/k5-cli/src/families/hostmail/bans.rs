//! The mail server's IP blocks: `hostmail bans` (list, clear, migrate and
//! its restore and status) and `hostmail allowlist` (list, add, remove),
//! as their `cmd_hostmail_*` functions read the words
//! (`cli/src/hostmail-bans-allowlist.sh`) and `_mail_py`'s verbs answer
//! (`cli/src/mail.sh`).

use super::pyx::{self, g, items, or_s, s_of};
use super::words::{scan, wants_help, Spec};
use crate::families::mail::core::{dumps, obj, raise_at, run_env, usage, At, Plan, Py, MODULE, R};
use crate::families::mail::py::{self as p, py_index};
use crate::out::json::Value;
use crate::out::py;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["bans", "allowlist"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const LIST_CALL: u32 = 2780;
    pub const LIST_BITS: u32 = 2789;
    pub const LIST_COUNT: u32 = 2790;
    pub const LIST_AUTH: u32 = 2791;
    pub const CLEAR_CALL: u32 = 2812;
    pub const CLEAR_JOIN: u32 = 2818;
    pub const CLEAR_WARN: u32 = 2820;
    pub const MIGRATE_INT: u32 = 2826;
    pub const MIGRATE_CALL: u32 = 2827;
    pub const RESTORE_CALL: u32 = 2840;
    pub const STATUS_CALL: u32 = 2848;
    pub const ALLOW_LIST_CALL: u32 = 2857;
    pub const ALLOW_LIST_FOR: u32 = 2864;
    pub const ALLOW_LIST_ROW: u32 = 2866;
    pub const ALLOW_ADD_CALL: u32 = 2877;
    pub const ALLOW_REMOVE_CALL: u32 = 2888;
    pub const ALLOW_REMOVE_JOIN: u32 = 2894;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "bans" => bans(words),
        "allowlist" => allowlist(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// `--json` only: `flag` for any other `-` word, `word` for the rest.
fn only_json(words: &[&str], flag: &str, word: &str) -> Result<bool, Plan> {
    scan(
        words,
        &Spec {
            slots: &[],
            values: &[],
            switches: &[],
            unknown: flag,
            extra: word,
        },
    )
    .map(|g| g.json)
}

/// `cmd_hostmail_bans`.
fn bans(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/bans"));
    };
    match *sub {
        "list" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/bans/list"));
            }
            let use_ = format!("usage: {CLI_NAME} hostmail bans list [--json]");
            let json = only_json(
                rest,
                &format!("unknown flag '{{w}}' — {use_}"),
                &format!("unexpected argument '{{w}}' — {use_}"),
            )?;
            Ok(Plan::Run(run_env("bans_list", json)))
        }
        "clear" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/bans/clear"));
            }
            let g = scan(
                rest,
                &Spec {
                    slots: &["ip"],
                    values: &[],
                    switches: &[("--all", "all")],
                    unknown: &format!(
                        "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail bans clear <ip> [--all] [--json]"
                    ),
                    extra: "unexpected extra argument '{w}' — one IP only",
                },
            )?;
            if g.get("ip").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail bans clear <ip> [--all]")));
            }
            Ok(Plan::Run(
                run_env("bans_clear", g.json)
                    .with("M_IP", g.get("ip"))
                    .with("M_ALL", if g.on("all") { "true" } else { "false" }),
            ))
        }
        "migrate" => migrate(rest),
        "--help" | "-h" => Ok(Plan::Help("hostmail/bans")),
        other => Err(usage(format!(
            "unknown bans command '{other}' — usage: {CLI_NAME} hostmail bans <list|clear|migrate>"
        ))),
    }
}

/// `cmd_hostmail_bans_migrate` (and its start, restore and status).
fn migrate(words: &[&str]) -> Result<Plan, Plan> {
    let Some(first) = words.first() else {
        return Ok(Plan::Help("hostmail/bans/migrate"));
    };
    match *first {
        "restore" | "status" => {
            let rest = &words[1..];
            if wants_help(rest) {
                return Ok(Plan::Help(if *first == "restore" {
                    "hostmail/bans/migrate/restore"
                } else {
                    "hostmail/bans/migrate/status"
                }));
            }
            let json = only_json(
                rest,
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail bans migrate {first} [--json]"
                ),
                "unexpected argument '{w}'",
            )?;
            Ok(Plan::Run(run_env(&format!("bans_migrate_{first}"), json)))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/bans/migrate")),
        _ => {
            if wants_help(words) {
                return Ok(Plan::Help("hostmail/bans/migrate"));
            }
            let g = scan(
                words,
                &Spec {
                    slots: &[],
                    values: &[("--hours", "hours", "--hours requires N")],
                    switches: &[],
                    unknown: &format!(
                        "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail bans migrate --hours N [--json]"
                    ),
                    extra: &format!(
                        "unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail bans migrate --hours N"
                    ),
                },
            )?;
            let hours = g.get("hours");
            if hours.is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail bans migrate --hours N")));
            }
            if !hours.bytes().all(|b| b.is_ascii_digit()) {
                return Err(usage("--hours must be a positive integer"));
            }
            if hours == "0" {
                return Err(usage("--hours must be >= 1"));
            }
            Ok(Plan::Run(
                run_env("bans_migrate", g.json).with("M_HOURS", hours),
            ))
        }
    }
}

/// `cmd_hostmail_allowlist`.
fn allowlist(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/allowlist"));
    };
    match *sub {
        "list" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/allowlist/list"));
            }
            let json = only_json(
                rest,
                &format!(
                    "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail allowlist list [--json]"
                ),
                "unexpected argument '{w}'",
            )?;
            Ok(Plan::Run(run_env("allowlist_list", json)))
        }
        "add" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/allowlist/add"));
            }
            let g = scan(
                rest,
                &Spec {
                    slots: &["address"],
                    values: &[
                        ("--reason", "reason", "--reason requires a value"),
                        ("--expires", "expires", "--expires requires RFC3339"),
                    ],
                    switches: &[],
                    unknown: &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail allowlist add <ip|cidr> [--reason migration] [--expires <rfc3339>] [--json]"),
                    extra: "unexpected extra argument '{w}'",
                },
            )?;
            if g.get("address").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail allowlist add <ip|cidr>")));
            }
            Ok(Plan::Run(
                run_env("allowlist_add", g.json)
                    .with("M_ADDRESS", g.get("address"))
                    .with("M_REASON", g.get("reason"))
                    .with("M_EXPIRES", g.get("expires")),
            ))
        }
        "remove" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/allowlist/remove"));
            }
            let g = scan(
                rest,
                &Spec {
                    slots: &["address"],
                    values: &[],
                    switches: &[],
                    unknown: &format!(
                        "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail allowlist remove <ip|cidr> [--json]"
                    ),
                    extra: "unexpected extra argument '{w}'",
                },
            )?;
            if g.get("address").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail allowlist remove <ip|cidr>"
                )));
            }
            Ok(Plan::Run(
                run_env("allowlist_remove", g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/allowlist")),
        other => Err(usage(format!(
            "unknown allowlist command '{other}' — usage: {CLI_NAME} hostmail allowlist <list|add|remove>"
        ))),
    }
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "bans_list" => list_py(py_),
        "bans_clear" => clear_py(py_),
        "bans_migrate" => migrate_py(py_),
        "bans_migrate_restore" => restore_py(py_),
        "bans_migrate_status" => status_py(py_),
        "allowlist_list" => allow_list_py(py_),
        "allowlist_add" => allow_add_py(py_),
        "allowlist_remove" => allow_remove_py(py_),
        _ => return None,
    })
}

/// `x if x is not None else "null"`.
fn or_null(v: Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "null".into(),
        Some(v) => py::str_of(&v),
    }
}

/// `"%s=%s" % (k, counts[k]) for k in sorted(counts, key=…)`: the
/// reason `authFailure` first, then the rest in order.
fn count_bits(counts: &Value) -> R<Vec<String>> {
    let l = line::LIST_BITS;
    let keys: Vec<Value> = p::iter(counts).at(l, MODULE)?;
    let mut names: Vec<String> = Vec::new();
    for k in &keys {
        match k {
            Value::Str(t) => {
                let s = p::text(t);
                if !names.contains(&s) {
                    names.push(s);
                }
            }
            other => {
                // A list of numbers: what `counts[k]` does with one.
                return Err(raise_at(
                    l,
                    py_index(counts, &py::str_of(other))
                        .err()
                        .unwrap_or_else(|| "TypeError: '<' not supported".into()),
                ));
            }
        }
    }
    names.sort_by(|a, b| {
        let rank = |s: &str| u8::from(s != "authFailure");
        (rank(a), a.as_str()).cmp(&(rank(b), b.as_str()))
    });
    names
        .iter()
        .map(|k| {
            let v = py_index(counts, k).at(l, MODULE)?;
            Ok(format!("{k}={}", py::str_of(&v)))
        })
        .collect()
}

fn list_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/bans", &[], line::LIST_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let bans = p::or(g(&d, "bans", line::LIST_BITS - 5)?, Value::Array(vec![]));
    let counts = p::or(g(&d, "counts", line::LIST_BITS - 4)?, Value::Object(vec![]));
    if !py::truthy(&bans) {
        py_.print("no blocked IPs");
        return Ok(());
    }
    let bits = count_bits(&counts)?;
    let count = match g(&d, "count", line::LIST_COUNT)? {
        None | Some(Value::Null) => pyx::len(&bans, line::LIST_COUNT)?.to_string(),
        Some(v) => py::str_of(&v),
    };
    py_.print(&format!("{count} blocked  {}", bits.join("  ")));
    let rows = p::iter(&bans).at(line::LIST_AUTH, MODULE)?;
    let mut auth = Vec::new();
    let mut other = Vec::new();
    for b in rows {
        let reason = or_s(g(&b, "reason", line::LIST_AUTH)?, "");
        if reason == "authFailure" {
            auth.push(b);
        } else {
            other.push((b, reason));
        }
    }
    if !auth.is_empty() {
        py_.print("authFailure:");
        for b in &auth {
            let l = line::LIST_AUTH + 6;
            py_.print(&format!(
                "  {}  id={}  expires={}",
                or_s(g(b, "address", l)?, "?"),
                or_s(g(b, "id", l + 1)?, "?"),
                or_null(g(b, "expiresAt", l + 2)?)
            ));
        }
    }
    let nscan = other.iter().filter(|(_, r)| r == "portScanning").count();
    let nrest = other.len() - nscan;
    if nscan > 0 || nrest > 0 {
        let mut extra = Vec::new();
        if nscan > 0 {
            extra.push(format!("{nscan} portScanning"));
        }
        if nrest > 0 {
            extra.push(format!("{nrest} other"));
        }
        py_.print(&format!(
            "scanners: {}  (full list: {CLI_NAME} hostmail bans list --json)",
            extra.join(", ")
        ));
    }
    Ok(())
}

/// `",".join(v or []) or "(none)"`, raised at `l`.
fn joined_or_none(v: Option<Value>, l: u32) -> R<String> {
    let v = p::or(v, Value::Array(vec![]));
    let j = pyx::join(",", &v, l)?;
    Ok(if j.is_empty() { "(none)".into() } else { j })
}

/// `d.get(key) or env(env_key)`.
fn or_env_bare(py_: &Py, d: &Value, key: &str, env_key: &str, l: u32) -> R<String> {
    Ok(match g(d, key, l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => py_.env(env_key).to_string(),
    })
}

fn clear_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("ip", pyx::env_v(py_, "M_IP")),
        ("all", Value::Bool(py_.env("M_ALL") == "true")),
    ]);
    let d = py_.post("/cli/mail/bans/clear", &body, line::CLEAR_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let ip = or_env_bare(py_, &d, "ip", "M_IP", line::CLEAR_JOIN - 1)?;
    let destroyed = joined_or_none(g(&d, "destroyed", line::CLEAR_JOIN)?, line::CLEAR_JOIN)?;
    let all = s_of(g(&d, "all", line::CLEAR_JOIN + 1)?);
    py_.print(&format!(
        "cleared ip={ip} destroyed={destroyed} all={all}"
    ));
    for w in items(g(&d, "warnings", line::CLEAR_WARN)?, line::CLEAR_WARN)? {
        py_.print(&format!("warn: {}", py::str_of(&w)));
    }
    py_.print("note: unban does not reset fail counters — pair with allowlist add");
    Ok(())
}

fn migrate_py(py_: &mut Py) -> R<()> {
    let hours = py_.env("M_HOURS").to_string();
    let body = if hours.is_empty() {
        obj(vec![])
    } else {
        let n = pyx::int(&pyx::env_v(py_, "M_HOURS"), line::MIGRATE_INT)?;
        obj(vec![("hours", Value::Int(n))])
    };
    let d = py_.post("/cli/mail/bans/migrate", &body, line::MIGRATE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::MIGRATE_CALL + 4;
    if p::truthy(g(&d, "active", l)?.as_ref()) && !hours.is_empty() {
        let h = or_s(g(&d, "hours", l + 2)?, &hours);
        let at = s_of(g(&d, "restoreAt", l + 2)?);
        py_.print(&format!(
            "migrate active hours={h} restoreAt={at} authBanRate=null"
        ));
        let snap = g(&d, "authBanRateSnapshot", l + 3)?.unwrap_or(Value::Null);
        py_.print(&format!("snapshot={}", dumps(&snap)));
    } else {
        let active = s_of(g(&d, "active", l + 6)?);
        let rem = s_of(g(&d, "remainingSeconds", l + 6)?);
        let at = s_of(g(&d, "restoreAt", l + 6)?);
        py_.print(&format!(
            "migrate active={active} remaining={rem}s restoreAt={at}"
        ));
    }
    Ok(())
}

fn restore_py(py_: &mut Py) -> R<()> {
    let d = py_.post("/cli/mail/bans/migrate/restore", &obj(vec![]), line::RESTORE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::RESTORE_CALL + 5;
    let restored = s_of(g(&d, "restored", l)?);
    let rate = g(&d, "authBanRate", l)?.unwrap_or(Value::Null);
    py_.print(&format!(
        "migrate restore restored={restored} authBanRate={}",
        dumps(&rate)
    ));
    Ok(())
}

fn status_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/bans/migrate", &[], line::STATUS_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::STATUS_CALL + 5;
    let active = s_of(g(&d, "active", l)?);
    let rem = s_of(g(&d, "remainingSeconds", l)?);
    let at = s_of(g(&d, "restoreAt", l + 1)?);
    let overdue = s_of(g(&d, "overdue", l + 1)?);
    py_.print(&format!(
        "migrate active={active} remaining={rem}s restoreAt={at} overdue={overdue}"
    ));
    Ok(())
}

fn allow_list_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/allowlist", &[], line::ALLOW_LIST_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let rows = p::or(
        g(&d, "allowlist", line::ALLOW_LIST_FOR - 3)?,
        Value::Array(vec![]),
    );
    if !py::truthy(&rows) {
        py_.print("allowlist empty");
        return Ok(());
    }
    for b in p::iter(&rows).at(line::ALLOW_LIST_FOR, MODULE)? {
        let l = line::ALLOW_LIST_ROW;
        let a = or_s(g(&b, "address", l)?, "?");
        let reason = or_s(g(&b, "reason", l + 1)?, "?");
        let id = or_s(g(&b, "id", l + 2)?, "?");
        let exp = or_null(g(&b, "expiresAt", l + 3)?);
        py_.print(&format!("{a}  reason={reason}  id={id}  expires={exp}"));
    }
    Ok(())
}

fn allow_add_py(py_: &mut Py) -> R<()> {
    let mut body = vec![("address", pyx::env_v(py_, "M_ADDRESS"))];
    if !py_.env("M_REASON").is_empty() {
        body.push(("reason", pyx::env_v(py_, "M_REASON")));
    }
    if !py_.env("M_EXPIRES").is_empty() {
        body.push(("expiresAt", pyx::env_v(py_, "M_EXPIRES")));
    }
    let d = py_.post("/cli/mail/allowlist/add", &obj(body), line::ALLOW_ADD_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::ALLOW_ADD_CALL + 5;
    let a = or_env_bare(py_, &d, "address", "M_ADDRESS", l)?;
    let id = or_s(g(&d, "id", l + 1)?, "?");
    let reason = or_s(g(&d, "reason", l + 2)?, "?");
    let exp = or_null(g(&d, "expiresAt", l + 3)?);
    py_.print(&format!(
        "allowlist add {a} id={id} reason={reason} expires={exp}"
    ));
    Ok(())
}

fn allow_remove_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("address", pyx::env_v(py_, "M_ADDRESS"))]);
    let d = py_.post("/cli/mail/allowlist/remove", &body, line::ALLOW_REMOVE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = or_env_bare(py_, &d, "address", "M_ADDRESS", line::ALLOW_REMOVE_JOIN - 1)?;
    let destroyed = joined_or_none(
        g(&d, "destroyed", line::ALLOW_REMOVE_JOIN)?,
        line::ALLOW_REMOVE_JOIN,
    )?;
    py_.print(&format!("allowlist remove {a} destroyed={destroyed}"));
    Ok(())
}
