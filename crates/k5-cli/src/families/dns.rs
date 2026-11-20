//! `k5 dns access|list|records|record add|record remove|verify`, as the
//! shell CLI's `dns` arm and its `cmd_dns_*` answer them
//! (`cli/src/router.sh`, `cli/src/dns.sh`): the zones and records this
//! box may manage through the daemon. `k5 dns` and `k5 dns record` alone
//! (or with an empty verb) print their help after the gates, as the shell
//! does. The frame is [`super::domain::dom`]'s; the reader is `_dns_handle_resp`.
//!
//! A `--ttl` or `--priority` of more than 4300 digits is the shell's:
//! its Python can't read a number that long.

use super::domain::dom::{self, at, first_steps, is_str, or_s, s, truthy_at, Act, Out};
use super::{py_iter, py_len, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{Value, MAX_INT_DIGITS};
use crate::out::py;
use crate::transport::encode::quote;
use k2_client::brand::{CLI_NAME, PUBLIC_SITE};
use k2_client::hosts::ZONE;
use std::ffi::OsString;

/// The routes.
pub const ACCESS: &str = "/cli/dns/access";
pub const ZONES: &str = "/cli/dns/zones";
pub const RECORDS: &str = "/cli/dns/records";
pub const RECORD_ADD: &str = "/cli/dns/records/add";
pub const RECORD_REMOVE: &str = "/cli/dns/records/remove";
pub const VERIFY: &str = "/cli/dns/verify";

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["dns"];
/// The binary answers it ([`run`] hands some lines back to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 dns ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    dom::run(args, parse, read)
}

/// The words after `dns`.
pub fn parse(words: &[&str]) -> Act {
    let rest = words.get(1..).unwrap_or_default();
    let get = |endpoint, query, json, verb| Act::Get {
        endpoint,
        query,
        json,
        verb,
    };
    match words.first().copied().unwrap_or("") {
        "access" => match dom::json_only(rest, &format!("{CLI_NAME} dns access [--json]")) {
            Ok(json) => get(ACCESS, Vec::new(), json, "access"),
            Err(hint) => Act::Usage(hint),
        },
        "list" => match dom::json_only(rest, &format!("{CLI_NAME} dns list [--json]")) {
            Ok(json) => get(ZONES, Vec::new(), json, "list"),
            Err(hint) => Act::Usage(hint),
        },
        "records" => match dom::one_arg(
            rest,
            &format!("{CLI_NAME} dns records <domain> [--json]"),
            " \u{2014} one <domain> only",
        ) {
            Err(hint) => Act::Usage(hint),
            Ok((d, _)) if d.is_empty() => {
                Act::Usage("records requires a <domain> (daemon resolves the zone)".to_string())
            }
            Ok((d, json)) => get(
                RECORDS,
                vec![format!("domain={}", quote(&d))],
                json,
                "records",
            ),
        },
        "record" => record(rest),
        "verify" => match dom::one_arg(
            rest,
            &format!("{CLI_NAME} dns verify <domain> [--json]"),
            " \u{2014} one <domain> only",
        ) {
            Err(hint) => Act::Usage(hint),
            Ok((d, _)) if d.is_empty() => Act::Usage("verify requires a <domain>".to_string()),
            Ok((d, json)) => Act::Post {
                endpoint: VERIFY,
                body: dom::body(vec![("domain", s(&d))]),
                json,
                verb: "verify",
            },
        },
        "" => Act::Help("dns"),
        other => Act::Usage(format!(
            "unknown command '{other}' \u{2014} see {CLI_NAME} dns --help"
        )),
    }
}

/// `cmd_dns_record`.
fn record(words: &[&str]) -> Act {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "add" => record_add(rest),
        "remove" | "rm" | "delete" | "del" => match dom::one_arg(
            rest,
            &format!("{CLI_NAME} dns record remove <record-id> [--json]"),
            " \u{2014} one <record-id> only",
        ) {
            Err(hint) => Act::Usage(hint),
            Ok((id, _)) if id.is_empty() => Act::Usage(format!(
                "record remove requires a <record-id> (from '{CLI_NAME} dns records <domain>')"
            )),
            Ok((id, json)) => Act::Post {
                endpoint: RECORD_REMOVE,
                body: dom::body(vec![("id", s(&id))]),
                json,
                verb: "remove",
            },
        },
        "" => Act::Help("dns/record"),
        other => Act::Usage(format!(
            "unknown record command '{other}' \u{2014} use add | remove (see {CLI_NAME} dns record --help)"
        )),
    }
}

/// `_dns_is_uint`: digits only, and not empty.
fn uint(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `int(t)` of digits: no leading zeros (`0` for all zeros).
fn int(digits: &str) -> Value {
    let t = digits.trim_start_matches('0');
    Value::Int(if t.is_empty() { "0".into() } else { t.into() })
}

/// `cmd_dns_record_add`.
fn record_add(rest: &[&str]) -> Act {
    let mut positionals: Vec<&str> = Vec::new();
    let mut ttl = "";
    let mut priority = "";
    let mut json = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            flag @ ("--ttl" | "--priority" | "--prio") => {
                let Some(v) = rest.get(i + 1) else {
                    let what = if flag == "--ttl" { "--ttl" } else { "--priority" };
                    return Act::Usage(format!("{what} requires a number"));
                };
                if flag == "--ttl" {
                    ttl = *v;
                } else {
                    priority = *v;
                }
                i += 2;
                continue;
            }
            "--json" => json = true,
            w if w.starts_with('-') => {
                return Act::Usage(format!(
                    "unknown flag '{w}' \u{2014} usage: {CLI_NAME} dns record add <domain> <type> <name> <value> [--ttl N] [--priority N] [--json]"
                ))
            }
            w => positionals.push(w),
        }
        i += 1;
    }
    if positionals.len() < 4 {
        return Act::Usage("record add requires <domain> <type> <name> <value>".to_string());
    }
    if let Some(extra) = positionals.get(4) {
        return Act::Usage(format!(
            "unexpected extra argument '{extra}' \u{2014} quote multi-word values"
        ));
    }
    if !ttl.is_empty() && !uint(ttl) {
        return Act::Usage("--ttl must be a non-negative integer".to_string());
    }
    if !priority.is_empty() && !uint(priority) {
        return Act::Usage("--priority must be a non-negative integer".to_string());
    }
    if ttl.len() > MAX_INT_DIGITS || priority.len() > MAX_INT_DIGITS {
        return Act::Shell;
    }
    let mut pairs = vec![
        ("domain", s(positionals[0])),
        ("type", s(positionals[1])),
        ("name", s(positionals[2])),
        ("value", s(positionals[3])),
    ];
    if !ttl.is_empty() {
        pairs.push(("ttl", int(ttl)));
    }
    if !priority.is_empty() {
        pairs.push(("priority", int(priority)));
    }
    Act::Post {
        endpoint: RECORD_ADD,
        body: dom::body(pairs),
        json,
        verb: "add",
    }
}

/// The lines of `_dns_handle_resp`'s Python, as `python3 -` numbers them
/// (`tests` pins them).
pub mod line {
    pub const DIE_PENDING: u32 = 51;
    pub const DIE_GATED: u32 = 53;
    pub const DIE_GATED_JOIN: u32 = 54;
    pub const ERR: u32 = 76;
    pub const DIE_ERR: u32 = 82;
    pub const ACCESS_WS: u32 = 100;
    pub const ACCESS_TYPES: u32 = 104;
    pub const ACCESS_LEN: u32 = 106;
    pub const ACCESS_FOR: u32 = 107;
    pub const LIST_FOR: u32 = 125;
    pub const LIST_NS: u32 = 137;
    pub const RECORDS_FOR: u32 = 161;
    pub const VERIFY_FOR: u32 = 224;
}

/// The teaching text when the switch is off (`DNS_GATED_HINT`).
pub const GATED_HINT: &str = "this agent isn't allowed to manage DNS \u{2014} Settings \u{2192} Network \u{2192} Tunnel \u{2192} Policies \u{2192} \"Agents can change DNS records\", or Settings \u{2192} Workspaces / Agents \u{2192} <workspace> \u{2192} Permissions \u{2192} DNS \u{2192} \"Let this agent manage DNS\".";

/// `PENDING_NS_HINT`.
fn pending_ns_hint() -> String {
    format!(
        "Pending: this zone's nameservers don't point at {ZONE} yet \u{2014} set them at the registrar ({CLI_NAME} domain list shows which). The server re-checks by itself; to check now: {CLI_NAME} domain refresh <apex>"
    )
}

const GATED: &[&str] = &[
    "gated",
    "forbidden",
    "dns_manage_disabled",
    "owner_only",
    "dns_zone_not_attached",
];
const USAGE: &[&str] = &["usage", "type_not_allowed"];
const PENDING_NS: &[&str] = &["zone_pending_ns"];
const READONLY: &[&str] = &["zone_readonly"];

/// `die(err)` of `_dns_handle_resp`, called from line `from`.
fn die(o: &mut Out, err: &Value, from: u32) -> Result<Exit, Raise> {
    let code = at(err, "code").cloned().unwrap_or_else(|| s("error"));
    let mut hint = at(err, "hint").cloned().unwrap_or_else(|| s(""));
    let raise = |line: u32, exception: String| Raise::inside(from, line, "die", exception);
    let unhashable = |t: &str| {
        raise(
            line::DIE_PENDING,
            format!("TypeError: unhashable type: '{t}'"),
        )
    };
    let pending = dom::code_in(&code, PENDING_NS).map_err(unhashable)?;
    if pending {
        let Value::Str(t) = &hint else {
            return Err(raise(
                line::DIE_PENDING,
                format!(
                    "AttributeError: '{}' object has no attribute 'startswith'",
                    py::type_name(&hint)
                ),
            ));
        };
        if !py::str_of(&hint).starts_with("Pending:") {
            let tail = if py::truthy(&hint) && hint != code {
                format!(" \u{2014} {}", py::str_of(&Value::Str(t.clone())))
            } else {
                String::new()
            };
            hint = s(&format!("{}{tail}", pending_ns_hint()));
        }
    }
    let gated = dom::code_in(&code, GATED).unwrap_or(false);
    if gated {
        let has_settings = match &hint {
            Value::Str(_) => py::str_of(&hint).contains("Settings"),
            Value::Array(items) => items.iter().any(|i| is_str(Some(i), "Settings")),
            Value::Object(pairs) => super::get(pairs, "Settings").is_some(),
            other => {
                return Err(raise(
                    line::DIE_GATED,
                    format!(
                        "TypeError: argument of type '{}' is not iterable",
                        py::type_name(other)
                    ),
                ))
            }
        };
        if !has_settings {
            let head = match &hint {
                h if !py::truthy(h) => String::new(),
                Value::Str(_) => format!("{} \u{2014} ", py::str_of(&hint)),
                Value::Array(_) => {
                    return Err(raise(
                        line::DIE_GATED_JOIN,
                        "TypeError: can only concatenate list (not \"str\") to list".to_string(),
                    ))
                }
                other => {
                    return Err(raise(
                        line::DIE_GATED_JOIN,
                        format!(
                            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            hint = s(&format!("{head}{GATED_HINT}"));
        }
    }
    o.stderr.push_str(&dom::dumps(&dom::obj(vec![(
        "error",
        dom::obj(vec![("code", code.clone()), ("hint", hint)]),
    )])));
    o.stderr.push('\n');
    let is = |set: &[&str]| dom::code_in(&code, set).unwrap_or(false);
    Ok(if gated || pending || is(READONLY) {
        Exit::Refused
    } else if is(USAGE) {
        Exit::Usage
    } else {
        Exit::Error
    })
}

/// `_dns_handle_resp` (`cli/src/dns.sh`) on `raw`.
pub fn read(raw: &str, as_json: bool, verb: &str) -> Shown {
    let mut o = Out::default();
    let r = read_into(&mut o, raw, as_json, verb);
    o.end(r)
}

/// `x or []`.
fn or_list(v: Option<&Value>) -> Value {
    v.filter(|x| py::truthy(x))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()))
}

/// `a or b or default`.
fn or2(a: Option<&Value>, b: Option<&Value>, default: &str) -> String {
    match (a, b) {
        (Some(a), _) if py::truthy(a) => py::str_of(a),
        (_, Some(b)) if py::truthy(b) => py::str_of(b),
        _ => default.to_string(),
    }
}

/// `d.get(k)` when not falsy, else `d["zone"].get(k)` when `zone` is a
/// dict (`records`' and `verify`'s domain).
fn or_zone<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match at(d, key) {
        Some(v) if py::truthy(v) => Some(v),
        v => match at(d, "zone") {
            Some(z @ Value::Object(_)) => at(z, key),
            _ => v,
        },
    }
}

fn read_into(o: &mut Out, raw: &str, as_json: bool, verb: &str) -> Result<Exit, Raise> {
    let d = match first_steps(o, raw, die, (line::ERR, line::DIE_ERR))? {
        Ok(d) => d,
        Err(exit) => return Ok(exit),
    };
    let known = ["access", "list", "records", "add", "remove", "verify"];
    if as_json || !known.contains(&verb) {
        o.line(&if as_json {
            dom::dumps(&d)
        } else {
            raw.to_string()
        });
        return Ok(Exit::Ok);
    }
    let ljust = super::ljust;
    match verb {
        "access" => {
            // `is None`: missing or null.
            let mut allowed = at(&d, "allowed").filter(|v| **v != Value::Null).cloned();
            if allowed.is_none() {
                if let Some(c @ Value::Object(_)) = at(&d, "capability") {
                    allowed = at(c, "allowed").cloned();
                }
            }
            let allowed = allowed.as_ref().is_some_and(py::truthy);
            o.line(&format!(
                "dns manage : {}",
                if allowed { "allowed" } else { "DENIED" }
            ));
            if !allowed {
                o.line(&format!(
                    "hint       : {}",
                    or_s(at(&d, "hint"), GATED_HINT)
                ));
            }
            let ws = at(&d, "workspace").filter(|w| py::truthy(w));
            if let Some(ws) = ws {
                if !matches!(ws, Value::Object(_)) {
                    return Err(Raise::no_attribute(line::ACCESS_WS, ws, "get"));
                }
                if truthy_at(ws, "path") || truthy_at(ws, "id") {
                    o.line(&format!(
                        "workspace  : {}",
                        or2(at(ws, "path"), at(ws, "id"), "?")
                    ));
                }
            }
            let types = or_list(at(&d, "record_types"));
            if py::truthy(&types) {
                let all: Vec<String> = py_iter(&types, line::ACCESS_TYPES)?
                    .iter()
                    .map(py::str_of)
                    .collect();
                o.line(&format!("types      : {}", all.join(", ")));
            }
            let zones = or_list(at(&d, "zones"));
            o.line(&format!(
                "zones      : {}",
                py_len(&zones, line::ACCESS_LEN)?
            ));
            for z in py_iter(&zones, line::ACCESS_FOR)? {
                if matches!(z, Value::Object(_)) {
                    o.line(&format!(
                        "  {}  {}",
                        ljust(&or_s(at(&z, "domain"), "?"), 28),
                        or_s(at(&z, "id"), "")
                    ));
                } else {
                    o.line(&format!("  {}", py::str_of(&z)));
                }
            }
        }
        "list" => {
            let zones = or_list(at(&d, "zones"));
            if !py::truthy(&zones) {
                o.line("(no DNS zones)");
                o.line(&format!(
                    "hint: add a zone in the {} Connect dashboard ({PUBLIC_SITE}), then delegate NS to ns1.{ZONE} / ns2.{ZONE} \u{2014} agents cannot create zones",
                    k2_client::brand::PRODUCT_NAME
                ));
                return Ok(Exit::Ok);
            }
            o.line(&format!(
                "{}  {}  ID",
                ljust("DOMAIN", 28),
                ljust("STATUS", 12)
            ));
            for z in py_iter(&zones, line::LIST_FOR)? {
                if !matches!(z, Value::Object(_)) {
                    o.line(&py::str_of(&z));
                    continue;
                }
                let status = match (at(&z, "status"), at(&z, "state")) {
                    (Some(v), _) if py::truthy(v) => v.clone(),
                    (_, Some(v)) if py::truthy(v) => v.clone(),
                    _ => s("?"),
                };
                o.line(&format!(
                    "{}  {}  {}",
                    ljust(&or_s(at(&z, "domain"), "?"), 28),
                    ljust(&py::str_of(&status), 12),
                    or_s(at(&z, "id"), "")
                ));
                let st = Some(&status);
                if is_str(st, "pending_ns") || is_str(st, "pending") || is_str(st, "error") {
                    let ns: Vec<String> = py_iter(&or_list(at(&z, "nameservers")), line::LIST_NS)?
                        .iter()
                        .filter(|n| py::truthy(n))
                        .map(py::str_of)
                        .collect();
                    let to = if ns.is_empty() {
                        format!("the {ZONE} nameservers ({CLI_NAME} domain list shows them)")
                    } else {
                        ns.join(", ")
                    };
                    o.line(&format!(
                        "  Pending: point nameservers to {to} (record writes refused until active; this server re-checks by itself)"
                    ));
                }
            }
        }
        "records" => {
            // `is None`: missing or null.
            let mut recs = at(&d, "records").filter(|v| **v != Value::Null).cloned();
            if recs.is_none() {
                if let Some(z @ Value::Object(_)) = at(&d, "zone") {
                    recs = at(z, "records").cloned();
                }
            }
            let recs = recs.unwrap_or(Value::Array(Vec::new()));
            o.line(&format!("zone: {}", or_s(or_zone(&d, "domain"), "?")));
            if !py::truthy(&recs) {
                o.line("(no records)");
                return Ok(Exit::Ok);
            }
            o.line(&format!(
                "{}  {}  {}  VALUE",
                ljust("TYPE", 6),
                ljust("NAME", 24),
                ljust("TTL", 8)
            ));
            for r in py_iter(&recs, line::RECORDS_FOR)? {
                if !matches!(r, Value::Object(_)) {
                    o.line(&py::str_of(&r));
                    continue;
                }
                let val = or2(at(&r, "content"), at(&r, "value"), "");
                let set = |k: &str| at(&r, k).filter(|v| **v != Value::Null);
                let extra = match (set("prio"), set("priority")) {
                    (Some(p), _) | (None, Some(p)) => format!(" (prio {})", py::str_of(p)),
                    _ => String::new(),
                };
                let ttl = set("ttl").map_or_else(|| "?".to_string(), py::str_of);
                let mut line = format!(
                    "{}  {}  {}  {val}{extra}",
                    ljust(&or_s(at(&r, "type"), "?"), 6),
                    ljust(&or_s(at(&r, "name"), "@"), 24),
                    ljust(&ttl, 8)
                );
                if truthy_at(&r, "id") {
                    line.push_str(&format!("  [{}]", or_s(at(&r, "id"), "")));
                }
                if let Some(mb) = at(&r, "managed_by") {
                    if py::truthy(mb) && !is_str(Some(mb), "user") {
                        line.push_str(&format!("  (managed_by={})", py::str_of(mb)));
                    }
                }
                o.line(&line);
            }
        }
        "add" => {
            let rec = match at(&d, "record") {
                Some(r @ Value::Object(_)) => r,
                _ => &d,
            };
            let rid = or2(at(rec, "id"), at(&d, "id"), "?");
            o.line(&format!(
                "added {} {} \u{2192} {}  (id {rid})",
                or_s(at(rec, "type"), "?"),
                or_s(at(rec, "name"), "@"),
                or2(at(rec, "content"), at(rec, "value"), "")
            ));
            if truthy_at(&d, "propagation") {
                o.line(&format!("propagation: {}", or_s(at(&d, "propagation"), "")));
            }
        }
        "remove" => {
            o.line(&format!(
                "removed record {}",
                or2(at(&d, "id"), at(&d, "record_id"), "?")
            ));
        }
        _ => {
            // verify
            let status = or2(
                at(&d, "status"),
                at(&d, "state"),
                if truthy_at(&d, "ok") { "ok" } else { "?" },
            );
            match or_zone(&d, "domain").filter(|v| py::truthy(v)) {
                Some(domain) => o.line(&format!("verify {}: {status}", py::str_of(domain))),
                None => o.line(&format!("verify: {status}")),
            }
            if truthy_at(&d, "hint") {
                o.line(&format!("hint: {}", or_s(at(&d, "hint"), "")));
            }
            let details = match (at(&d, "checks"), at(&d, "results")) {
                (Some(v), _) if py::truthy(v) => v.clone(),
                (_, Some(v)) if py::truthy(v) => v.clone(),
                _ => Value::Array(Vec::new()),
            };
            for c in py_iter(&details, line::VERIFY_FOR)? {
                if matches!(c, Value::Object(_)) {
                    let first = or2(at(&c, "status"), at(&c, "ok"), "?");
                    let name = match (at(&c, "name"), at(&c, "check")) {
                        (Some(v), _) if py::truthy(v) => py::str_of(v),
                        (_, Some(v)) if py::truthy(v) => py::str_of(v),
                        _ => dom::dumps(&c),
                    };
                    o.line(&format!("  {} {name}", ljust(&first, 12)));
                } else {
                    o.line(&format!("  {}", py::str_of(&c)));
                }
            }
        }
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests;
