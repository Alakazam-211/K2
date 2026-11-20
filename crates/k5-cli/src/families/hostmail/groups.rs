//! Groups and shared calendars: `hostmail group` (create, add, remove,
//! list, show, delete) and `hostmail calendar` (status, enable, disable,
//! create, rename, share, unshare, shares, sharing), as their
//! `cmd_hostmail_*` functions read the words
//! (`cli/src/hostmail-bans-allowlist.sh`, `cli/src/hostmail-dkim-dmarc.sh`)
//! and `_mail_py`'s verbs answer (`cli/src/mail.sh`).

use super::pyx::{self, g, items, or_dict, or_s, s_of, sv};
use super::words::wants_help;
use crate::families::mail::core::{
    is_uint, obj, run_env, usage, At, Plan, Py, Secs, Within, MODULE, R,
};
use crate::families::mail::py::{self as p, attr_get};
use crate::out::json::Value;
use crate::out::py;
use crate::transport::urllib::Method;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["group", "calendar"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const DAV_GET: u32 = 3080;
    pub const DAV_POST: u32 = 3085;
    pub const DAV_POLICY: u32 = 3089;
    pub const DAV_ACC: u32 = 3105;
    pub const DAV_ACC_GET: u32 = 3108;
    pub const DAV_SKIPPED: u32 = 3109;
    pub const PG_HEAD: u32 = 3118;
    pub const PG_MAIL: u32 = 3119;
    pub const PG_FOR: u32 = 3120;
    pub const CREATE_CALL: u32 = 3126;
    pub const MEMBERS_CALL: u32 = 3136;
    pub const MEMBERS_JOIN: u32 = 3138;
    pub const LIST_CALL: u32 = 3140;
    pub const LIST_FOR: u32 = 3147;
    pub const LIST_PRINT: u32 = 3148;
    pub const SHOW_CALL: u32 = 3150;
    pub const SHOW_PRINT: u32 = 3154;
    pub const DELETE_CALL: u32 = 3156;
    pub const DELETE_PRINT: u32 = 3158;
    pub const MANAGE_CALL: u32 = 3168;
    pub const MANAGE_C: u32 = 3172;
    pub const MANAGE_CREATED: u32 = 3174;
    pub const MANAGE_RENAMED: u32 = 3177;
    pub const SHARE_CALL: u32 = 3184;
    pub const SHARE_C: u32 = 3189;
    pub const SHARE_PRINT: u32 = 3193;
    pub const SHARE_REMOVED: u32 = 3198;
    pub const SHARE_NOTHING: u32 = 3201;
    pub const SHARES_CALL: u32 = 3203;
    pub const SHARES_FOR: u32 = 3210;
    pub const SHARES_SHARES: u32 = 3211;
    pub const SHARES_ROW: u32 = 3212;
    pub const SHARES_DEFAULT: u32 = 3213;
    pub const SHARES_LEN: u32 = 3214;
    pub const SHARES_EACH: u32 = 3215;
    pub const SHARES_EACH_ROW: u32 = 3216;
    pub const SHARING_POST: u32 = 3221;
    pub const SHARING_INT: u32 = 3222;
    pub const SHARING_GET: u32 = 3224;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "group" => group(words),
        "calendar" => calendar(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// `cmd_hostmail_group`.
fn group(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/group"));
    };
    match *sub {
        "create" | "add" | "remove" | "list" | "show" | "delete" => {}
        "--help" | "-h" => return Ok(Plan::Help("hostmail/group")),
        other => {
            return Err(usage(format!(
                "unknown group command '{other}' — usage: {CLI_NAME} hostmail group <create|add|remove|list|show|delete>"
            )))
        }
    }
    if wants_help(rest) {
        return Ok(Plan::Help("hostmail/group"));
    }
    let (mut json, mut mail, mut domain) = (false, false, String::new());
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--json" => json = true,
            "--mail" => {
                if *sub != "create" {
                    return Err(usage("--mail goes with 'group create'"));
                }
                mail = true;
            }
            "--domain" => {
                if *sub != "create" {
                    return Err(usage("--domain goes with 'group create'"));
                }
                domain = rest
                    .get(i + 1)
                    .ok_or_else(|| usage("--domain requires a value"))?
                    .to_string();
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(format!(
                    "unknown flag '{f}' — see {CLI_NAME} hostmail group --help"
                )))
            }
            w => pos.push(w),
        }
        i += 1;
    }
    let n = pos.len();
    match *sub {
        "create" => {
            if n != 1 {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail group create <name> [--domain <apex>] [--mail]"
                )));
            }
            Ok(Plan::Run(
                run_env("group_create", json)
                    .with("M_NAME", pos[0])
                    .with("M_DOMAIN", domain)
                    .with("M_GROUP_MAIL", if mail { "true" } else { "false" }),
            ))
        }
        "add" | "remove" => {
            if n < 2 {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail group {sub} <group> <addr> [<addr>…]"
                )));
            }
            Ok(Plan::Run(
                run_env("group_members", json)
                    .with("M_GROUP", pos[0])
                    .with("M_LIST_OP", *sub)
                    .with("M_MEMBERS", pos[1..].join("\n")),
            ))
        }
        "list" => {
            if n != 0 {
                return Err(usage(format!("{CLI_NAME} hostmail group list [--json]")));
            }
            Ok(Plan::Run(run_env("group_list", json)))
        }
        _ => {
            if n != 1 {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail group {sub} <group> [--json]"
                )));
            }
            Ok(Plan::Run(
                run_env(&format!("group_{sub}"), json).with("M_GROUP", pos[0]),
            ))
        }
    }
}

/// `cmd_hostmail_calendar`.
fn calendar(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/calendar"));
    };
    match *sub {
        "status" | "enable" | "disable" => calendar_action(sub, rest),
        "create" | "rename" | "share" | "unshare" | "shares" | "sharing" => {
            calendar_share(sub, rest)
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/calendar")),
        other => Err(usage(format!(
            "unknown calendar command '{other}' — usage: {CLI_NAME} hostmail calendar <status|enable|disable|create|rename|share|unshare|shares|sharing>"
        ))),
    }
}

/// `cmd_hostmail_calendar_action`.
fn calendar_action(action: &str, words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help(match action {
            "status" => "hostmail/calendar/status",
            "enable" => "hostmail/calendar/enable",
            _ => "hostmail/calendar/disable",
        }));
    }
    let use_ = format!("usage: {CLI_NAME} hostmail calendar {action} [--files on|off] [--json]");
    let (mut json, mut files) = (false, String::new());
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => json = true,
            "--files" => {
                let v = words
                    .get(i + 1)
                    .ok_or_else(|| usage(format!("--files requires on|off — {use_}")))?;
                match *v {
                    "on" | "off" => files = v.to_string(),
                    other => {
                        return Err(usage(format!("--files must be on or off (got '{other}')")))
                    }
                }
                i += 1;
            }
            w => return Err(usage(format!("unexpected argument '{w}' — {use_}"))),
        }
        i += 1;
    }
    if action == "status" && !files.is_empty() {
        return Err(usage("--files goes with enable|disable, not status"));
    }
    Ok(Plan::Run(
        run_env("dav", json)
            .with("M_DAV_ACTION", action)
            .with("M_FILES", files),
    ))
}

/// `cmd_hostmail_calendar_share`.
fn calendar_share(sub: &str, words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/calendar/share"));
    }
    let allowed: &[&str] = match sub {
        "create" | "rename" => &["--color", "--json"],
        "share" => &[
            "--calendar",
            "--with",
            "--read",
            "--write",
            "--editor",
            "--freebusy",
            "--json",
        ],
        "unshare" => &["--calendar", "--with", "--json"],
        "shares" => &["--json"],
        _ => &["--max-shares", "--json"],
    };
    let see = format!("see {CLI_NAME} hostmail calendar --help");
    let mut json = false;
    let (mut calendar, mut with, mut level, mut color, mut max) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w.starts_with("--") {
            if !allowed.contains(&w) {
                return Err(usage(format!(
                    "'{CLI_NAME} hostmail calendar {sub}' does not take '{w}' — {see}"
                )));
            }
            match w {
                "--json" => {
                    json = true;
                    i += 1;
                    continue;
                }
                "--read" | "--write" | "--editor" | "--freebusy" => {
                    if !level.is_empty() {
                        return Err(usage("give ONE of --freebusy --read --write --editor"));
                    }
                    level = w[2..].to_string();
                    i += 1;
                    continue;
                }
                _ => {}
            }
            let v = words
                .get(i + 1)
                .ok_or_else(|| usage(format!("{w} requires a value — {see}")))?
                .to_string();
            match w {
                "--calendar" => calendar = v,
                "--with" => with = v,
                "--color" => color = v,
                _ => max = v,
            }
            i += 2;
            continue;
        }
        if w.starts_with('-') {
            return Err(usage(format!("unknown flag '{w}' — {see}")));
        }
        pos.push(w);
        i += 1;
    }
    let n = pos.len();
    let env = match sub {
        "create" => {
            if n != 2 {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail calendar create <owner-addr> <name> [--color <c>]"
                )));
            }
            run_env("cal_manage", json)
                .with("M_CAL_ACTION", "create")
                .with("M_ADDRESS", pos[0])
                .with("M_NAME", pos[1])
                .with("M_COLOR", color)
        }
        "rename" => {
            if n != 3 {
                return Err(usage(format!("{CLI_NAME} hostmail calendar rename <owner-addr> <calendar> <new-name> [--color <c>]")));
            }
            run_env("cal_manage", json)
                .with("M_CAL_ACTION", "rename")
                .with("M_ADDRESS", pos[0])
                .with("M_CALENDAR", pos[1])
                .with("M_NAME", pos[2])
                .with("M_COLOR", color)
        }
        "share" | "unshare" => {
            if n != 1 {
                return Err(usage(format!("{CLI_NAME} hostmail calendar {sub} <owner-addr> [--calendar <id|name>] --with <addr|group>")));
            }
            if with.is_empty() {
                return Err(usage(format!("{sub} needs --with <addr|group>")));
            }
            if sub == "share" && level.is_empty() {
                return Err(usage(
                    "share needs one of --freebusy --read --write --editor",
                ));
            }
            run_env(&format!("cal_{sub}"), json)
                .with("M_ADDRESS", pos[0])
                .with("M_CALENDAR", calendar)
                .with("M_WITH", with)
                .with("M_SHARE_LEVEL", level)
        }
        "shares" => {
            if n != 1 {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail calendar shares <owner-addr> [--json]"
                )));
            }
            run_env("cal_shares", json).with("M_ADDRESS", pos[0])
        }
        _ => {
            if n != 0 {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail calendar sharing [--max-shares N] [--json]"
                )));
            }
            if !max.is_empty() && !is_uint(&max) {
                return Err(usage("--max-shares must be a number from 1 to 1000"));
            }
            run_env("cal_sharing", json).with("M_MAX_SHARES", max)
        }
    };
    Ok(Plan::Run(env))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "dav" => dav_py(py_),
        "group_create" => group_create_py(py_),
        "group_members" => group_members_py(py_),
        "group_list" => group_list_py(py_),
        "group_show" => group_show_py(py_),
        "group_delete" => group_delete_py(py_),
        "cal_manage" => cal_manage_py(py_),
        "cal_share" | "cal_unshare" => cal_share_py(py_, verb == "cal_share"),
        "cal_shares" => cal_shares_py(py_),
        "cal_sharing" => cal_sharing_py(py_),
        _ => return None,
    })
}

/// `call(method, route, params=…, body=…, timeout=secs)` from line `l`.
fn call(
    py_: &Py,
    method: Method,
    route: &str,
    params: &[(String, String)],
    body: Option<&Value>,
    secs: u64,
    l: u32,
) -> R<Value> {
    py_.call_full(method, route, params, body, Secs::Int(secs), None, l)
}

fn on_off(v: Option<Value>) -> &'static str {
    if p::truthy(v.as_ref()) {
        "on"
    } else {
        "off"
    }
}

fn dav_py(py_: &mut Py) -> R<()> {
    let action = py_.env("M_DAV_ACTION").to_string();
    let d = if action == "status" {
        py_.get("/cli/mail/dav", &[], line::DAV_GET)?
    } else {
        let mut body = vec![("action", sv(&action))];
        if !py_.env("M_FILES").is_empty() {
            body.push(("files", pyx::env_v(py_, "M_FILES")));
        }
        call(
            py_,
            Method::Post,
            "/cli/mail/dav",
            &[],
            Some(&obj(body)),
            300,
            line::DAV_POST,
        )?
    };
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DAV_POLICY;
    let pol = g(&d, "policy", l)?;
    let policy = if pol.as_ref().is_some_and(|v| p::eq_str(v, "default")) {
        "Stalwart default (never set — run enable or disable once)".to_string()
    } else if pol.as_ref().is_some_and(|v| p::eq_str(v, "applied")) {
        let mut s = "set by the owner".to_string();
        if matches!(g(&d, "backfilledAt", l + 4)?, None | Some(Value::Null)) {
            s.push_str("; some accounts not done yet (finished on the next daemon start)");
        }
        s
    } else {
        or_s(pol, "?")
    };
    py_.print(&format!(
        "calendars : {}  (CalDAV + CardDAV for every hosted address)",
        on_off(g(&d, "enabled", l + 8)?)
    ));
    py_.print(&format!(
        "files     : {}  (WebDAV file storage)",
        on_off(g(&d, "files", l + 9)?)
    ));
    py_.print(&format!("policy    : {policy}"));
    let reach = if p::truthy(g(&d, "reachable", l + 11)?.as_ref()) {
        "yes"
    } else {
        "no"
    };
    py_.print(&format!("reachable : {reach}"));
    if let Some(u) = g(&d, "url", l + 12)?.filter(py::truthy) {
        py_.print(&format!("url       : {}", py::str_of(&u)));
    }
    if let Some(r) = g(&d, "reason", l + 14)?.filter(py::truthy) {
        py_.print(&format!("reason    : {}", py::str_of(&r)));
    }
    if let Some(acc @ Value::Object(_)) = g(&d, "accounts", line::DAV_ACC)? {
        let la = line::DAV_ACC_GET;
        let get =
            |k: &str| -> R<String> { Ok(py::str_of(&p::get_or(&acc, k, sv("?")).at(la, MODULE)?)) };
        let (total, updated, unchanged) = (get("total")?, get("updated")?, get("unchanged")?);
        let skipped = p::or(g(&acc, "skipped", line::DAV_SKIPPED)?, Value::Array(vec![]));
        let n = pyx::len(&skipped, line::DAV_SKIPPED)?;
        py_.print(&format!(
            "accounts  : {total} total, {updated} updated, {unchanged} unchanged, {n} skipped"
        ));
    }
    py_.print(&format!("note      : a calendar app retrying a wrong password gets its IP banned — {CLI_NAME} hostmail bans list / {CLI_NAME} hostmail allowlist"));
    Ok(())
}

/// `print_group(g)`, called from module line `from`.
fn print_group(py_: &mut Py, grp: &Value, from: u32) -> R<()> {
    const F: &str = "print_group";
    let r = (|| -> R<()> {
        let at = |v: Result<Option<Value>, String>, l: u32| v.at(l, F);
        let l = line::PG_HEAD;
        let name = match at(attr_get(grp, "address"), l)?.filter(py::truthy) {
            Some(v) => py::str_of(&v),
            None => or_s(at(attr_get(grp, "name"), l)?, "?"),
        };
        let members = p::or(at(attr_get(grp, "members"), l)?, Value::Array(vec![]));
        let n = p::len(&members).at(l, F)?;
        let mail = if p::truthy(at(attr_get(grp, "receivesMail"), line::PG_MAIL)?.as_ref()) {
            ""
        } else {
            " · inbound mail off"
        };
        py_.print(&format!("group {name} · {n} member(s){mail}"));
        let members = p::or(
            at(attr_get(grp, "members"), line::PG_FOR)?,
            Value::Array(vec![]),
        );
        for m in p::iter(&members).at(line::PG_FOR, F)? {
            py_.print(&format!("  {}", py::str_of(&m)));
        }
        Ok(())
    })();
    r.within(from, MODULE)
}

fn group_create_py(py_: &mut Py) -> R<()> {
    let mut body = vec![
        ("name", pyx::env_v(py_, "M_NAME")),
        ("mail", Value::Bool(py_.env("M_GROUP_MAIL") == "true")),
    ];
    if !py_.env("M_DOMAIN").is_empty() {
        body.push(("domain", pyx::env_v(py_, "M_DOMAIN")));
    }
    let d = call(
        py_,
        Method::Post,
        "/cli/mail/group",
        &[],
        Some(&obj(body)),
        60,
        line::CREATE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::CREATE_CALL + 4;
    let a = s_of(g(&d, "address", l)?);
    let mail = if p::truthy(g(&d, "receivesMail", l + 1)?.as_ref()) {
        ""
    } else {
        " (inbound mail off; --mail keeps it on)"
    };
    py_.print(&format!("created group {a}{mail}"));
    py_.print(&format!(
        "add members: {CLI_NAME} hostmail group add {a} <addr> [<addr>…]"
    ));
    Ok(())
}

fn group_members_py(py_: &mut Py) -> R<()> {
    let key = if py_.env("M_LIST_OP") == "add" {
        "add"
    } else {
        "remove"
    };
    let members: Vec<Value> = py_.env.list("M_MEMBERS").iter().map(|m| sv(m)).collect();
    let body = obj(vec![
        ("group", pyx::env_v(py_, "M_GROUP")),
        (key, Value::Array(members)),
    ]);
    let d = call(
        py_,
        Method::Post,
        "/cli/mail/group/members",
        &[],
        Some(&body),
        120,
        line::MEMBERS_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::MEMBERS_JOIN;
    let grp = match g(&d, "group", l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => py_.env("M_GROUP").to_string(),
    };
    let m = p::or(g(&d, "members", l)?, Value::Array(vec![]));
    let j = pyx::join(",", &m, l)?;
    let j = if j.is_empty() { "(none)".into() } else { j };
    py_.print(&format!("group {grp} members={j}"));
    Ok(())
}

fn group_list_py(py_: &mut Py) -> R<()> {
    let d = call(
        py_,
        Method::Get,
        "/cli/mail/group",
        &[],
        None,
        60,
        line::LIST_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let gs = p::or(g(&d, "groups", line::LIST_CALL + 4)?, Value::Array(vec![]));
    if !py::truthy(&gs) {
        py_.print(&format!(
            "No groups. Create one: {CLI_NAME} hostmail group create <name>"
        ));
        return Ok(());
    }
    for grp in p::iter(&gs).at(line::LIST_FOR, MODULE)? {
        print_group(py_, &grp, line::LIST_PRINT)?;
    }
    Ok(())
}

fn group_show_py(py_: &mut Py) -> R<()> {
    let params = vec![("group".to_string(), py_.env("M_GROUP").to_string())];
    let d = call(
        py_,
        Method::Get,
        "/cli/mail/group",
        &params,
        None,
        60,
        line::SHOW_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let grp = or_dict(g(&d, "group", line::SHOW_PRINT)?);
    print_group(py_, &grp, line::SHOW_PRINT)
}

fn group_delete_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("group", pyx::env_v(py_, "M_GROUP"))]);
    let d = call(
        py_,
        Method::Post,
        "/cli/mail/group/delete",
        &[],
        Some(&body),
        300,
        line::DELETE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DELETE_PRINT;
    let grp = match g(&d, "group", l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => py_.env("M_GROUP").to_string(),
    };
    let zero = Value::Int("0".into());
    let shares = py::str_of(&p::get_or(&d, "sharesRemoved", zero.clone()).at(l, MODULE)?);
    let members = py::str_of(&p::get_or(&d, "membersRemoved", zero).at(l, MODULE)?);
    py_.print(&format!(
        "deleted group {grp} ({shares} calendar share(s) and {members} membership(s) removed)"
    ));
    Ok(())
}

fn cal_manage_py(py_: &mut Py) -> R<()> {
    let mut body = vec![
        ("action", pyx::env_v(py_, "M_CAL_ACTION")),
        ("owner", pyx::env_v(py_, "M_ADDRESS")),
        ("name", pyx::env_v(py_, "M_NAME")),
    ];
    if !py_.env("M_CALENDAR").is_empty() {
        body.push(("calendar", pyx::env_v(py_, "M_CALENDAR")));
    }
    if !py_.env("M_COLOR").is_empty() {
        body.push(("color", pyx::env_v(py_, "M_COLOR")));
    }
    let d = call(
        py_,
        Method::Post,
        "/cli/mail/calendar/manage",
        &[],
        Some(&obj(body)),
        60,
        line::MANAGE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let c = or_dict(g(&d, "calendar", line::MANAGE_C)?);
    if p::truthy(g(&d, "created", line::MANAGE_C + 1)?.as_ref()) {
        let l = line::MANAGE_CREATED;
        let (name, id) = (s_of(g(&c, "name", l)?), s_of(g(&c, "id", l)?));
        let owner = s_of(g(&d, "owner", l)?);
        py_.print(&format!("created calendar '{name}' ({id}) on {owner}"));
    } else {
        let l = line::MANAGE_RENAMED;
        let prev = s_of(g(&c, "previousName", l)?);
        let (name, id) = (s_of(g(&c, "name", l)?), s_of(g(&c, "id", l)?));
        let owner = s_of(g(&d, "owner", l)?);
        py_.print(&format!(
            "renamed calendar '{prev}' → '{name}' ({id}) on {owner}"
        ));
    }
    Ok(())
}

fn cal_share_py(py_: &mut Py, share: bool) -> R<()> {
    let mut body = vec![
        ("owner", pyx::env_v(py_, "M_ADDRESS")),
        ("with", pyx::env_v(py_, "M_WITH")),
    ];
    if !py_.env("M_CALENDAR").is_empty() {
        body.push(("calendar", pyx::env_v(py_, "M_CALENDAR")));
    }
    if share {
        body.push(("level", pyx::env_v(py_, "M_SHARE_LEVEL")));
    }
    let route = if share {
        "/cli/mail/calendar/share"
    } else {
        "/cli/mail/calendar/unshare"
    };
    let d = call(
        py_,
        Method::Post,
        route,
        &[],
        Some(&obj(body)),
        120,
        line::SHARE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let c = or_dict(g(&d, "calendar", line::SHARE_C)?);
    let who = |l: u32| -> R<(String, String, String)> {
        Ok((
            s_of(g(&c, "name", l)?),
            s_of(g(&d, "owner", l)?),
            s_of(g(&d, "with", l)?),
        ))
    };
    if share {
        let prev = g(&d, "previousLevel", line::SHARE_PRINT - 2)?;
        let (name, owner, with) = who(line::SHARE_PRINT)?;
        let level = s_of(g(&d, "level", line::SHARE_PRINT)?);
        let was = match prev.filter(py::truthy) {
            Some(v) => format!(" (was {})", py::str_of(&v)),
            None => String::new(),
        };
        py_.print(&format!(
            "shared '{name}' of {owner} with {with}: {level}{was}"
        ));
        if p::truthy(g(&d, "maxShares", line::SHARE_PRINT + 2)?.as_ref()) {
            let max = s_of(g(&d, "maxShares", line::SHARE_PRINT + 3)?);
            py_.print(&format!(
                "limit: {max} shares per calendar ({CLI_NAME} hostmail calendar sharing)"
            ));
        }
    } else if p::truthy(g(&d, "removed", line::SHARE_REMOVED - 1)?.as_ref()) {
        let (name, owner, with) = who(line::SHARE_REMOVED)?;
        py_.print(&format!("stopped sharing '{name}' of {owner} with {with}"));
    } else {
        let (name, owner, with) = who(line::SHARE_NOTHING)?;
        py_.print(&format!(
            "'{name}' of {owner} was not shared with {with} — nothing to do"
        ));
    }
    Ok(())
}

fn cal_shares_py(py_: &mut Py) -> R<()> {
    let params = vec![("owner".to_string(), py_.env("M_ADDRESS").to_string())];
    let d = call(
        py_,
        Method::Get,
        "/cli/mail/calendar/shares",
        &params,
        None,
        60,
        line::SHARES_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let cals = p::or(
        g(&d, "calendars", line::SHARES_FOR - 3)?,
        Value::Array(vec![]),
    );
    if !py::truthy(&cals) {
        py_.print(&format!(
            "{} has no calendars.",
            s_of(g(&d, "owner", line::SHARES_FOR - 1)?)
        ));
    }
    let cals = if py::truthy(&cals) {
        p::iter(&cals).at(line::SHARES_FOR, MODULE)?
    } else {
        Vec::new()
    };
    for c in cals {
        let shares = p::or(g(&c, "shares", line::SHARES_SHARES)?, Value::Array(vec![]));
        let l = line::SHARES_ROW;
        let id = s_of(g(&c, "id", l)?);
        let name = or_s(g(&c, "name", l)?, "(unnamed)");
        let def = if p::truthy(g(&c, "isDefault", line::SHARES_DEFAULT)?.as_ref()) {
            " · default"
        } else {
            ""
        };
        let shared = if py::truthy(&shares) {
            format!("{} share(s)", pyx::len(&shares, line::SHARES_LEN)?)
        } else {
            "not shared".into()
        };
        py_.print(&format!("{id} · {name}{def} · {shared}"));
        for s in items(Some(shares), line::SHARES_EACH)? {
            let l = line::SHARES_EACH_ROW;
            py_.print(&format!(
                "  {} ({}): {}",
                s_of(g(&s, "with", l)?),
                s_of(g(&s, "withType", l)?),
                s_of(g(&s, "level", l)?)
            ));
        }
    }
    if p::truthy(g(&d, "maxShares", line::SHARES_EACH_ROW + 1)?.as_ref()) {
        let max = s_of(g(&d, "maxShares", line::SHARES_EACH_ROW + 2)?);
        py_.print(&format!("limit: {max} shares per calendar"));
    }
    Ok(())
}

fn cal_sharing_py(py_: &mut Py) -> R<()> {
    let d = if py_.env("M_MAX_SHARES").is_empty() {
        call(
            py_,
            Method::Get,
            "/cli/mail/calendar/sharing",
            &[],
            None,
            60,
            line::SHARING_GET,
        )?
    } else {
        let n = pyx::int(&pyx::env_v(py_, "M_MAX_SHARES"), line::SHARING_INT)?;
        call(
            py_,
            Method::Post,
            "/cli/mail/calendar/sharing",
            &[],
            Some(&obj(vec![("maxShares", Value::Int(n))])),
            60,
            line::SHARING_POST,
        )?
    };
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::SHARING_GET + 4;
    py_.print(&format!(
        "max shares per calendar : {}",
        s_of(g(&d, "maxShares", l)?)
    ));
    py_.print(&format!(
        "directory queries       : {}",
        on_off(g(&d, "allowDirectoryQueries", l + 1)?)
    ));
    if let Some(Value::Str(t)) = g(&d, "reloaded", l + 2)? {
        py_.print(&format!("note: {}", p::text(&t)));
    }
    Ok(())
}
