//! Hosted mail forwards: `mail forward` (a person's own mailbox, the
//! `/cli/mail/self/forward*` routes) as `cmd_mail_forward` reads its words,
//! the words `_forward_set_parse` reads for it and for `hostmail forward`
//! (`cli/src/hostmail-app-cert-backup.sh`), and `_mail_py`'s `forward_*`
//! and `self_forward_*` branch (`cli/src/mail.sh`), which both share.

use super::core::{self, run_env, usage, At, End, Env, Plan, Py, Secs, Within, MODULE, R};
use super::py::{self as p, attr_get, iter};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::ops::py_eq;
use crate::transport::urllib::Method;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// The lines of `_mail_py` where the forward branch calls or raises.
/// `tests` pins each to its statement.
pub mod line {
    pub const DIE_CODE: u32 = 1213;
    pub const DIE_HINT: u32 = 1214;
    pub const DIE_ANY: u32 = 1229;
    pub const CALL_REQUEST: u32 = 1232;
    pub const CALL_ERROR: u32 = 1235;
    pub const CALL_DIE: u32 = 1236;
    pub const CALL_OK: u32 = 1237;
    pub const LINE_TO: u32 = 1250;
    pub const LINE_KIND: u32 = 1251;
    pub const LINE_STATE: u32 = 1252;
    pub const DETAIL_STATE: u32 = 1258;
    pub const DETAIL_WORDS: u32 = 1261;
    pub const DETAIL_REASON: u32 = 1263;
    pub const DETAIL_HELD: u32 = 1264;
    pub const DETAIL_CANT: u32 = 1278;
    pub const DETAIL_NOTICE_WORDS: u32 = 1280;
    pub const DETAIL_NOTICE: u32 = 1282;
    pub const DETAIL_TOLD: u32 = 1283;
    pub const DETAIL_C: u32 = 1284;
    pub const DETAIL_ANY: u32 = 1285;
    pub const DETAIL_COUNTS: u32 = 1287;
    pub const DETAIL_SET_BY: u32 = 1288;
    pub const NOTE: u32 = 1293;
    pub const GET_CALL: u32 = 1298;
    pub const GET_A: u32 = 1302;
    pub const GET_ROWS: u32 = 1303;
    pub const GET_FORWARD: u32 = 1305;
    pub const GET_FOR: u32 = 1309;
    pub const GET_LINE: u32 = 1310;
    pub const GET_DETAIL: u32 = 1311;
    pub const GET_VOL: u32 = 1313;
    pub const GET_VOL_PRINT: u32 = 1316;
    pub const GET_EVS: u32 = 1317;
    pub const GET_EV_FOR: u32 = 1320;
    pub const GET_EV_DETAIL: u32 = 1321;
    pub const GET_EV_PRINT: u32 = 1325;
    pub const SET_CALL: u32 = 1341;
    pub const SET_A: u32 = 1345;
    pub const SET_FOR: u32 = 1346;
    pub const SET_TO: u32 = 1347;
    pub const SET_KEEP: u32 = 1348;
    pub const SET_PRINT: u32 = 1349;
    pub const SET_MESSAGE: u32 = 1350;
    pub const SET_NOTE: u32 = 1352;
    pub const SET_NOTE_LINE: u32 = 1354;
    pub const RM_CALL: u32 = 1361;
    pub const RM_A: u32 = 1365;
    pub const RM_PRINT: u32 = 1366;
    pub const RM_LEFT: u32 = 1367;
    pub const RM_LEFT_PRINT: u32 = 1368;
    pub const RM_NOTE_LINE: u32 = 1369;
    pub const UNSET_CALL: u32 = 1373;
    pub const UNSET_A: u32 = 1377;
    pub const UNSET_REMOVED: u32 = 1378;
    pub const UNSET_NOTE_LINE: u32 = 1382;
    pub const PAUSE_CALL: u32 = 1388;
    pub const PAUSE_A: u32 = 1392;
    pub const PAUSE_ROWS: u32 = 1393;
    pub const PAUSE_WHICH: u32 = 1394;
    pub const PAUSE_STATE: u32 = 1398;
    pub const PAUSE_SKIPPED: u32 = 1399;
    pub const PAUSE_SK: u32 = 1400;
    pub const PAUSE_NOTE_LINE: u32 = 1401;
    pub const LIST_CALL: u32 = 1409;
    pub const LIST_ROWS: u32 = 1413;
    pub const LIST_OC: u32 = 1414;
    pub const LIST_OC_OK: u32 = 1415;
    pub const LIST_OC_ERROR: u32 = 1416;
    pub const LIST_FOR: u32 = 1419;
    pub const LIST_LINE: u32 = 1420;
    pub const LIST_PAUSED: u32 = 1421;
    pub const LIST_CANT: u32 = 1423;
    pub const LIST_NOTICE: u32 = 1425;
}

/// The verbs (second words) of `mail` this module answers.
pub const VERBS: &[&str] = &["forward"];

/// The words after `mail <verb>` for [`VERBS`].
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    match verb {
        "forward" => parse_mail(words),
        _ => Plan::Shell,
    }
}

/// Who runs `_forward_set_parse`: the address is a word for `hostmail`,
/// `--address` for `mail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Mail,
    Hostmail,
}

/// `_wants_help "$@"`.
pub fn wants_help(words: &[&str]) -> bool {
    words.iter().any(|w| *w == "--help" || *w == "-h")
}

/// `_forward_set_parse <family> <op> …`: the settings of `set`, `add` and
/// `rm` (`M_ADDRESS`, `M_TO` one address a line, `M_KEEP_ARG`,
/// `M_RESTART`, `M_JSON`), or the usage line.
pub fn parse_set(family: Family, op: &str, words: &[&str], verb: &str) -> Result<Env, Plan> {
    let what = if op == "rm" {
        "--to <dest> [--to <dest>…]"
    } else {
        "--to <dest> [--to <dest>…] [--keep|--no-keep] [--restart-stopped]"
    };
    let use_ = match family {
        Family::Hostmail => format!("{CLI_NAME} hostmail forward {op} <addr> {what} [--json]"),
        Family::Mail => format!("{CLI_NAME} mail forward {op} {what} [--address <addr>] [--json]"),
    };
    let (mut address, mut to, mut keep, mut json, mut restart) =
        (String::new(), String::new(), "", false, false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => json = true,
            "--keep" | "--no-keep" | "--restart-stopped" => {
                if op == "rm" {
                    return Err(usage(format!("unknown flag '{w}' — usage: {use_}")));
                }
                match w {
                    "--keep" => {
                        if keep == "false" {
                            return Err(usage(format!(
                                "give --keep or --no-keep, not both — usage: {use_}"
                            )));
                        }
                        keep = "true";
                    }
                    "--no-keep" => {
                        if keep == "true" {
                            return Err(usage(format!(
                                "give --keep or --no-keep, not both — usage: {use_}"
                            )));
                        }
                        keep = "false";
                    }
                    _ => restart = true,
                }
            }
            "--to" => {
                let v = words
                    .get(i + 1)
                    .ok_or_else(|| usage(format!("--to requires an address — usage: {use_}")))?;
                if !to.is_empty() {
                    to.push('\n');
                }
                to.push_str(v);
                i += 1;
            }
            "--address" => {
                let v = words
                    .get(i + 1)
                    .ok_or_else(|| usage("--address requires a value"))?;
                address = (*v).to_string();
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(format!("unknown flag '{f}' — usage: {use_}")));
            }
            other => {
                if family == Family::Hostmail && address.is_empty() {
                    address = other.to_string();
                } else {
                    return Err(usage(format!(
                        "unexpected extra argument '{other}' — usage: {use_}"
                    )));
                }
            }
        }
        i += 1;
    }
    if family == Family::Hostmail && address.is_empty() {
        return Err(usage(use_));
    }
    if to.is_empty() {
        return Err(usage(format!(
            "forward {op} requires --to <dest> (a mailbox here, or an outside address) — usage: {use_}"
        )));
    }
    Ok(run_env(verb, json)
        .with("M_ADDRESS", address)
        .with("M_TO", to)
        .with("M_KEEP_ARG", keep)
        .with("M_RESTART", if restart { "true" } else { "false" }))
}

/// `cmd_mail_forward`: the words after `mail forward`.
pub fn parse_mail(words: &[&str]) -> Plan {
    const HELP: Plan = Plan::Help("mail/forward");
    let Some((sub, rest)) = words.split_first() else {
        return HELP;
    };
    match *sub {
        "set" | "add" | "rm" => {
            if wants_help(rest) {
                return HELP;
            }
            parse_set(Family::Mail, sub, rest, &format!("self_forward_{sub}"))
                .map_or_else(|p| p, Plan::Run)
        }
        "show" | "unset" | "pause" | "resume" => {
            if wants_help(rest) {
                return HELP;
            }
            simple(sub, rest).unwrap_or_else(|p| p)
        }
        "--help" | "-h" => HELP,
        other => usage(format!(
            "unknown forward command '{other}' — usage: {CLI_NAME} mail forward <set|add|rm|show|unset|pause|resume>"
        )),
    }
}

/// `mail forward show|unset|pause|resume [--to <dest>] [--address <addr>]
/// [--json]`.
fn simple(sub: &str, words: &[&str]) -> Result<Plan, Plan> {
    let use_ = match sub {
        "pause" | "resume" => {
            format!("{CLI_NAME} mail forward {sub} [--to <dest>] [--address <addr>] [--json]")
        }
        _ => format!("{CLI_NAME} mail forward {sub} [--address <addr>] [--json]"),
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
                if !matches!(sub, "pause" | "resume") {
                    return Err(usage(format!("unknown argument '--to' — usage: {use_}")));
                }
                to = words
                    .get(i + 1)
                    .ok_or_else(|| usage(format!("--to requires an address — usage: {use_}")))?
                    .to_string();
                i += 1;
            }
            other => {
                return Err(usage(format!("unknown argument '{other}' — usage: {use_}")));
            }
        }
        i += 1;
    }
    let verb = if sub == "show" {
        "self_forward_get".to_string()
    } else {
        format!("self_forward_{sub}")
    };
    Ok(Plan::Run(
        run_env(&verb, json)
            .with("M_ADDRESS", address)
            .with("M_TO", to),
    ))
}

/// The forward branch of `_mail_py`, for `M_VERB` `forward_*` or
/// `self_forward_*`. `None`: not this branch's verb.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    // `forward_message` is an earlier branch's.
    if verb == "forward_message" {
        return None;
    }
    let (is_self, op) = match verb.strip_prefix("self_forward_") {
        Some(op) => (true, op),
        None => (false, verb.strip_prefix("forward_")?),
    };
    let f = Fwd {
        op: op.to_string(),
        base: if is_self {
            "/cli/mail/self/forward"
        } else {
            "/cli/mail/forward"
        },
        addr: py_.env("M_ADDRESS").to_string(),
    };
    Some(f.run(py_))
}

/// The branch's locals: `op`, `base_route`, `addr_arg`.
struct Fwd {
    op: String,
    base: &'static str,
    addr: String,
}

const FWD_DIE: &str = "fwd_die";
const FWD_CALL: &str = "fwd_call";
const FWD_LINE: &str = "fwd_line";
const FWD_DETAIL: &str = "fwd_detail";
const NOTE_LINE: &str = "note_line";
const GENEXPR: &str = "<genexpr>";

/// `x.get(k)` raised at `l` of `name`.
fn g(x: &Value, k: &str, l: u32, name: &'static str) -> R<Option<Value>> {
    attr_get(x, k).at(l, name)
}

/// `x[k]` (known to be there) raised at `l` of `name`.
fn idx(x: &Value, k: &str, l: u32, name: &'static str) -> R<Value> {
    p::py_index(x, k).at(l, name)
}

/// `"%s" % v`.
fn s(v: &Value) -> String {
    py::str_of(v)
}

fn sv(t: &str) -> Value {
    p::sv(t)
}

fn is(v: &Option<Value>, t: &str) -> bool {
    v.as_ref().is_some_and(|v| p::eq_str(v, t))
}

/// `x or default` where `x` is a `get`.
fn or(v: Option<Value>, default: Value) -> Value {
    p::or(v, default)
}

/// `when(ts)`: `time.strftime("%Y-%m-%d %H:%M", time.localtime(int(ts)))`,
/// or `?` where any step raises.
fn when(ts: Option<&Value>) -> String {
    let Some(n) = ts.and_then(|v| p::py_int(v).ok()) else {
        return "?".into();
    };
    crate::families::app::py::local_time(&Value::Int(n), "%Y-%m-%d %H:%M", 0)
        .unwrap_or_else(|_| "?".into())
}

/// `evs[:5]`, or the exception's text.
fn first_five(v: &Value) -> Result<Value, String> {
    match v {
        Value::Array(items) => Ok(Value::Array(items.iter().take(5).cloned().collect())),
        // A string's slice is a string: its first letter's `get` raises,
        // whatever the letters are.
        Value::Str(_) => Ok(v.clone()),
        Value::Object(_) => Err("KeyError: slice(None, 5, None)".into()),
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// Adds the caller's frames, outermost first, to an exception.
fn inside(end: End, frames: &[(u32, &'static str)]) -> End {
    match end {
        End::Raise(mut t) => {
            for f in frames.iter().rev() {
                t.frames.insert(0, *f);
            }
            End::Raise(t)
        }
        other => other,
    }
}

impl Fwd {
    fn run(&self, py_: &mut Py) -> R<()> {
        match self.op.as_str() {
            "get" => self.get(py_),
            "set" | "add" => self.set(py_),
            "rm" => self.rm(py_),
            "unset" => self.unset(py_),
            "pause" | "resume" => self.pause(py_),
            "list" => self.list(py_),
            op => Err(py_.die_with("usage", &format!("unknown forward verb {op}"), None)),
        }
    }

    /// `fwd_die(err, permission)`, called from `fwd_call` (itself called
    /// from module line `from`).
    fn die(&self, py_: &Py, err: &Value, permission: Option<&Value>, from: u32) -> End {
        let frames = [(from, MODULE), (line::CALL_DIE, FWD_CALL)];
        let err_or = if py::truthy(err) {
            err.clone()
        } else {
            Value::Object(Vec::new())
        };
        let code = match p::get_or(&err_or, "code", sv("error")).at(line::DIE_CODE, FWD_DIE) {
            Ok(c) => c,
            Err(e) => return inside(e, &frames),
        };
        let hint = match p::get_or(&err_or, "hint", sv("")).at(line::DIE_HINT, FWD_DIE) {
            Ok(h) => h,
            Err(e) => return inside(e, &frames),
        };
        if p::eq_str(&code, "person_only") {
            return py_.die_with(
                "person_only",
                &format!(
                    "{CLI_NAME} mail forward is for a person's own mailbox (a box login, or the owner). \
                     An agent uses {CLI_NAME} hostmail forward <set|add|rm|show|unset|pause|resume> <address> \
                     (needs \"Let this agent manage hosted mail\" on)"
                ),
                Some(Exit::Refused),
            );
        }
        const OWN_HINT: &[&str] = &[
            "destination_stopped",
            "too_many_destinations",
            "new_outside_limit",
            "notice_limit",
            "suppressed",
            "no_front_door",
            "no_mail_path",
            "forward_loop",
        ];
        if OWN_HINT.iter().any(|c| p::eq_str(&code, c)) {
            return py_.die(
                &core::obj(vec![("code", code), ("hint", hint)]),
                Some(Exit::Error),
                None,
            );
        }
        if p::eq_str(&code, "owner_only") && self.op == "resume" {
            let line = core::dumps(&core::obj(vec![(
                "error",
                core::obj(vec![("code", code), ("hint", hint)]),
            )]));
            crate::out::stderr_line(&line);
            return End::Exit(Exit::Refused);
        }
        inside(
            py_.die(err, None, permission),
            &[
                (from, MODULE),
                (line::CALL_DIE, FWD_CALL),
                (line::DIE_ANY, FWD_DIE),
            ],
        )
    }

    /// `fwd_call(method, route, params, body, timeout)` from module line
    /// `from`.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &self,
        py_: &Py,
        method: Method,
        route: &str,
        params: &[(String, String)],
        body: Option<&Value>,
        timeout: u64,
        from: u32,
    ) -> R<Value> {
        let d = py_
            .request(method, route, params, body, Secs::Int(timeout))
            .within(line::CALL_REQUEST, FWD_CALL)
            .within(from, MODULE)?;
        let Some(d) = d else {
            return Err(py_.unreachable());
        };
        let err = g(&d, "error", line::CALL_ERROR, FWD_CALL).within(from, MODULE)?;
        if let Some(e) = err.filter(py::truthy) {
            let perm = attr_get(&d, "permission").ok().flatten();
            return Err(self.die(py_, &e, perm.as_ref(), from));
        }
        let ok = g(&d, "ok", line::CALL_OK, FWD_CALL).within(from, MODULE)?;
        if !p::truthy(ok.as_ref()) {
            let t: String = core::dumps(&d).chars().take(200).collect();
            return Err(py_.die_with("error", &format!("unexpected daemon response: {t}"), None));
        }
        Ok(d)
    }

    /// `d.get("address") or addr_arg or "?"` at `l`.
    fn a(&self, d: &Value, l: u32) -> R<String> {
        let v = g(d, "address", l, MODULE)?.filter(py::truthy);
        Ok(match v {
            Some(v) => s(&v),
            None if !self.addr.is_empty() => self.addr.clone(),
            None => "?".into(),
        })
    }

    /// `note_line(d)`, called from module line `from`.
    fn note_line(py_: &mut Py, d: &Value, from: u32) -> R<()> {
        let n = g(d, "note", line::NOTE, NOTE_LINE).within(from, MODULE)?;
        if let Some(n) = n.filter(py::truthy) {
            py_.print(&format!("note: {}", s(&n)));
        }
        Ok(())
    }

    /// `{"address": addr_arg} if addr_arg else {}`.
    fn addr_body(&self) -> Vec<(&'static str, Value)> {
        if self.addr.is_empty() {
            Vec::new()
        } else {
            vec![("address", sv(&self.addr))]
        }
    }

    fn get(&self, py_: &mut Py) -> R<()> {
        let params = if self.addr.is_empty() {
            Vec::new()
        } else {
            vec![("address".to_string(), self.addr.clone())]
        };
        let d = self.call(
            py_,
            Method::Get,
            self.base,
            &params,
            None,
            30,
            line::GET_CALL,
        )?;
        if py_.json {
            py_.print_json(&d);
            return Ok(());
        }
        let a = self.a(&d, line::GET_A)?;
        let rows = match g(&d, "forwards", line::GET_ROWS, MODULE)? {
            None | Some(Value::Null) => {
                if p::truthy(g(&d, "forward", line::GET_FORWARD, MODULE)?.as_ref()) {
                    Value::Array(vec![idx(&d, "forward", line::GET_FORWARD, MODULE)?])
                } else {
                    Value::Array(Vec::new())
                }
            }
            Some(v) => v,
        };
        if !py::truthy(&rows) {
            py_.print(&format!("{a}  forward=none"));
            return Ok(());
        }
        for f in iter(&rows).at(line::GET_FOR, MODULE)? {
            let l = fwd_line(&f).within(line::GET_LINE, MODULE)?;
            py_.print(&format!("{a}  {l}"));
            for l in fwd_detail(&f).within(line::GET_DETAIL, MODULE)? {
                py_.print(&l);
            }
        }
        let vol = or(
            g(&d, "volume", line::GET_VOL, MODULE)?,
            Value::Object(Vec::new()),
        );
        if py::truthy(&vol) {
            let l = line::GET_VOL_PRINT;
            let counted = p::get_or(&vol, "copiesCounted", Value::Int("0".into())).at(l, MODULE)?;
            let day = g(&vol, "copiesPerDay", l, MODULE)?.unwrap_or(Value::Null);
            let hour = g(&vol, "copiesPerHour", l, MODULE)?.unwrap_or(Value::Null);
            py_.print(&format!(
                "volume: {} of {} copies counted in 24 hours (at the limit {PRODUCT_NAME} pauses its outside forwards), {} an hour (above that the mail server holds them)",
                s(&counted),
                s(&day),
                s(&hour)
            ));
        }
        let evs = or(
            g(&d, "events", line::GET_EVS, MODULE)?,
            Value::Array(Vec::new()),
        );
        if py::truthy(&evs) {
            py_.print("  recent events (newest first):");
            let five = first_five(&evs).at(line::GET_EV_FOR, MODULE)?;
            for e in iter(&five).at(line::GET_EV_FOR, MODULE)? {
                let det = or(
                    g(&e, "detail", line::GET_EV_DETAIL, MODULE)?,
                    Value::Object(Vec::new()),
                );
                let mut extra = String::new();
                if let Value::Object(_) = det {
                    if let Some(r) = attr_get(&det, "reason").ok().flatten().filter(py::truthy) {
                        extra = format!("  ({})", s(&r));
                    }
                }
                let l = line::GET_EV_PRINT;
                let at = g(&e, "at", l, MODULE)?;
                let w = when(at.as_ref());
                let event = or(g(&e, "event", l, MODULE)?, sv("?"));
                let actor = or(g(&e, "actor", l, MODULE)?, sv("?"));
                py_.print(&format!(
                    "    {w}  {} {}{extra}",
                    p::ljust(&s(&event), 12),
                    s(&actor)
                ));
            }
        }
        Ok(())
    }

    /// `[t for t in env("M_TO").split("\n") if t]` and the `to` it sends.
    fn tos(py_: &Py) -> (Vec<String>, Value) {
        let tos: Vec<String> = py_
            .env("M_TO")
            .split('\n')
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        let to = if tos.len() == 1 {
            sv(&tos[0])
        } else {
            Value::Array(tos.iter().map(|t| sv(t)).collect())
        };
        (tos, to)
    }

    fn set(&self, py_: &mut Py) -> R<()> {
        let (tos, to) = Self::tos(py_);
        let mut body = vec![("to", to)];
        body.extend(self.addr_body());
        let keep = py_.env("M_KEEP_ARG");
        if keep == "true" || keep == "false" {
            body.push(("keep", Value::Bool(keep == "true")));
        }
        if py_.env("M_RESTART") == "true" {
            body.push(("restartStopped", Value::Bool(true)));
        }
        let route = if self.op == "set" {
            self.base.to_string()
        } else {
            format!("{}/add", self.base)
        };
        let d = self.call(
            py_,
            Method::Post,
            &route,
            &[],
            Some(&core::obj(body)),
            30 + 2 * tos.len() as u64,
            line::SET_CALL,
        )?;
        if py_.json {
            py_.print_json(&d);
            return Ok(());
        }
        let a = self.a(&d, line::SET_A)?;
        let dests = match g(&d, "destinations", line::SET_FOR, MODULE)?.filter(py::truthy) {
            Some(v) => v,
            None => Value::Array(vec![d.clone()]),
        };
        for one in iter(&dests).at(line::SET_FOR, MODULE)? {
            let dest = or(g(&one, "to", line::SET_TO, MODULE)?, sv("?"));
            let keep = if p::truthy(g(&one, "keep", line::SET_KEEP, MODULE)?.as_ref()) {
                "keeps a copy"
            } else {
                "no copy kept"
            };
            let state = or(g(&one, "state", line::SET_PRINT, MODULE)?, sv("active"));
            py_.print(&format!(
                "forward {a} -> {} ({keep}): {}",
                s(&dest),
                s(&state)
            ));
            if p::truthy(g(&one, "message", line::SET_MESSAGE, MODULE)?.as_ref()) {
                let m = idx(&one, "message", line::SET_MESSAGE + 1, MODULE)?;
                py_.print_v(&m);
            }
            let note = g(&one, "note", line::SET_NOTE, MODULE)?;
            if let Some(n) = note.filter(py::truthy) {
                let top = attr_get(&d, "note").ok().flatten().unwrap_or(Value::Null);
                if !py_eq(&n, &top) {
                    py_.print(&format!("note: {}", s(&n)));
                }
            }
        }
        Self::note_line(py_, &d, line::SET_NOTE_LINE)
    }

    fn rm(&self, py_: &mut Py) -> R<()> {
        let (tos, to) = Self::tos(py_);
        let mut body = vec![("to", to)];
        body.extend(self.addr_body());
        let d = self.call(
            py_,
            Method::Post,
            &format!("{}/rm", self.base),
            &[],
            Some(&core::obj(body)),
            30,
            line::RM_CALL,
        )?;
        if py_.json {
            py_.print_json(&d);
            return Ok(());
        }
        let a = self.a(&d, line::RM_A)?;
        let removed = or(
            g(&d, "removed", line::RM_PRINT, MODULE)?,
            Value::Array(tos.iter().map(|t| sv(t)).collect()),
        );
        let removed = p::join(", ", &removed).at(line::RM_PRINT, MODULE)?;
        py_.print(&format!("{a} no longer forwards to {removed}"));
        let left = or(
            g(&d, "remaining", line::RM_LEFT, MODULE)?,
            Value::Array(Vec::new()),
        );
        let left = if py::truthy(&left) {
            p::join(", ", &left).at(line::RM_LEFT_PRINT, MODULE)?
        } else {
            "nothing".into()
        };
        py_.print(&format!("still forwarding to: {left}"));
        Self::note_line(py_, &d, line::RM_NOTE_LINE)
    }

    fn unset(&self, py_: &mut Py) -> R<()> {
        let d = self.call(
            py_,
            Method::Post,
            &format!("{}/unset", self.base),
            &[],
            Some(&core::obj(self.addr_body())),
            30,
            line::UNSET_CALL,
        )?;
        if py_.json {
            py_.print_json(&d);
            return Ok(());
        }
        let a = self.a(&d, line::UNSET_A)?;
        if matches!(
            g(&d, "removed", line::UNSET_REMOVED, MODULE)?,
            Some(Value::Bool(false))
        ) {
            py_.print(&format!("{a} had no forward (nothing to remove)"));
        } else {
            py_.print(&format!("forward removed for {a}"));
        }
        Self::note_line(py_, &d, line::UNSET_NOTE_LINE)
    }

    fn pause(&self, py_: &mut Py) -> R<()> {
        let mut body = self.addr_body();
        if !py_.env("M_TO").is_empty() {
            body.push(("to", sv(py_.env("M_TO"))));
        }
        let d = self.call(
            py_,
            Method::Post,
            &format!("{}/{}", self.base, self.op),
            &[],
            Some(&core::obj(body)),
            30,
            line::PAUSE_CALL,
        )?;
        if py_.json {
            py_.print_json(&d);
            return Ok(());
        }
        let a = self.a(&d, line::PAUSE_A)?;
        let rows = or(
            g(&d, "forwards", line::PAUSE_ROWS, MODULE)?,
            Value::Array(Vec::new()),
        );
        let which = if py::truthy(&rows) {
            let l = line::PAUSE_WHICH;
            let mut tos = Vec::new();
            for r in iter(&rows).at(l, MODULE)? {
                tos.push(or(g(&r, "to", l, GENEXPR).within(l, MODULE)?, sv("?")));
            }
            format!(" ({})", p::join(", ", &Value::Array(tos)).at(l, MODULE)?)
        } else {
            String::new()
        };
        if self.op == "pause" {
            py_.print(&format!(
                "forward paused for {a}{which}: new mail stays in the mailbox"
            ));
        } else {
            let state = or(g(&d, "state", line::PAUSE_STATE, MODULE)?, sv("?"));
            py_.print(&format!(
                "forward resumed for {a}{which} (state {})",
                s(&state)
            ));
        }
        let skipped = or(
            g(&d, "skipped", line::PAUSE_SKIPPED, MODULE)?,
            Value::Array(Vec::new()),
        );
        for sk in iter(&skipped).at(line::PAUSE_SKIPPED, MODULE)? {
            let to = or(g(&sk, "to", line::PAUSE_SK, MODULE)?, sv("?"));
            let why = or(g(&sk, "why", line::PAUSE_SK, MODULE)?, sv("?"));
            py_.print(&format!("left alone: {}: {}", s(&to), s(&why)));
        }
        Self::note_line(py_, &d, line::PAUSE_NOTE_LINE)
    }

    fn list(&self, py_: &mut Py) -> R<()> {
        let mut params = Vec::new();
        for (k, e) in [("state", "M_STATE"), ("kind", "M_KIND")] {
            if !py_.env(e).is_empty() {
                params.push((k.to_string(), py_.env(e).to_string()));
            }
        }
        let d = self.call(
            py_,
            Method::Get,
            "/cli/mail/forwards",
            &params,
            None,
            30,
            line::LIST_CALL,
        )?;
        if py_.json {
            py_.print_json(&d);
            return Ok(());
        }
        let rows = or(
            g(&d, "forwards", line::LIST_ROWS, MODULE)?,
            Value::Array(Vec::new()),
        );
        let oc = or(
            g(&d, "outcomes", line::LIST_OC, MODULE)?,
            Value::Object(Vec::new()),
        );
        if matches!(
            g(&oc, "ok", line::LIST_OC_OK, MODULE)?,
            Some(Value::Bool(false))
        ) {
            let e = or(g(&oc, "error", line::LIST_OC_ERROR, MODULE)?, sv("unknown"));
            py_.print(&format!(
                "warning: delivery reports aren't reaching {PRODUCT_NAME} ({}), so forwards won't pause by themselves; {PRODUCT_NAME} retries hourly",
                s(&e)
            ));
        }
        if !py::truthy(&rows) {
            py_.print("no forwards");
        }
        for f in iter(&rows).at(line::LIST_FOR, MODULE)? {
            let addr = or(g(&f, "address", line::LIST_LINE, MODULE)?, sv("?"));
            let l = fwd_line(&f).within(line::LIST_LINE, MODULE)?;
            py_.print(&format!("{}  {l}", s(&addr)));
            if is(&g(&f, "state", line::LIST_PAUSED, MODULE)?, "paused")
                && p::truthy(g(&f, "pauseWords", line::LIST_PAUSED, MODULE)?.as_ref())
            {
                let w = idx(&f, "pauseWords", line::LIST_PAUSED + 1, MODULE)?;
                py_.print(&format!("  paused because {}", s(&w)));
            }
            if p::truthy(g(&f, "cantArrive", line::LIST_CANT, MODULE)?.as_ref()) {
                let w = idx(&f, "cantArrive", line::LIST_CANT + 1, MODULE)?;
                py_.print(&format!("  {}", s(&w)));
            }
            if p::truthy(g(&f, "noticeWords", line::LIST_NOTICE, MODULE)?.as_ref()) {
                let w = idx(&f, "noticeWords", line::LIST_NOTICE + 1, MODULE)?;
                py_.print(&format!("  {}", s(&w)));
            }
        }
        Ok(())
    }
}

/// `fwd_line(f)`.
fn fwd_line(f: &Value) -> R<String> {
    let to = or(g(f, "to", line::LINE_TO, FWD_LINE)?, sv("?"));
    let keep = p::truthy(g(f, "keep", line::LINE_TO, FWD_LINE)?.as_ref());
    let kind = or(g(f, "kind", line::LINE_KIND, FWD_LINE)?, sv("?"));
    let state = or(g(f, "state", line::LINE_KIND, FWD_LINE)?, sv("?"));
    let mut out = format!(
        "to={}  keep={}  kind={}  state={}",
        s(&to),
        if keep { "true" } else { "false" },
        s(&kind),
        s(&state)
    );
    if is(&g(f, "state", line::LINE_STATE, FWD_LINE)?, "paused")
        && p::truthy(g(f, "pauseReason", line::LINE_STATE, FWD_LINE)?.as_ref())
    {
        let r = idx(f, "pauseReason", line::LINE_STATE + 1, FWD_LINE)?;
        out.push_str(&format!("  reason={}", s(&r)));
    }
    Ok(out)
}

/// `fwd_detail(f)`.
fn fwd_detail(f: &Value) -> R<Vec<String>> {
    const F: &str = FWD_DETAIL;
    let mut out: Vec<String> = Vec::new();
    let paused = is(&g(f, "state", line::DETAIL_STATE, F)?, "paused");
    if paused
        && is(
            &g(f, "pauseReason", line::DETAIL_STATE, F)?,
            "upgrade_required",
        )
    {
        out.push(format!(
            "  not forwarding: the mail server needs upgrading: {CLI_NAME} hostmail upgrade"
        ));
        out.push("  it resumes by itself after the upgrade".into());
    } else if is(&g(f, "state", line::DETAIL_WORDS, F)?, "paused")
        && p::truthy(g(f, "pauseWords", line::DETAIL_WORDS, F)?.as_ref())
    {
        let w = idx(f, "pauseWords", line::DETAIL_WORDS + 1, F)?;
        out.push(format!("  paused because {}", s(&w)));
        let reason = or(g(f, "pauseReason", line::DETAIL_REASON, F)?, sv(""));
        if p::truthy(g(f, "heldBy", line::DETAIL_HELD, F)?.as_ref()) {
            out.push(format!(
                "  someone also paused it, so it stays paused when {PRODUCT_NAME} lets go; resume takes that back"
            ));
        }
        let r = |t: &str| p::eq_str(&reason, t);
        if r("not_told") {
            out.push(format!(
                "  it goes back on by itself when {PRODUCT_NAME} can email the address its stop link; or remove it"
            ));
        } else if r("agent_send_off") {
            out.push("  it goes back on when agent sending does; or the owner, an Admin or the mailbox's person resumes it, and it's theirs".into());
        } else if r("revoked") {
            out.push("  it can't be resumed; only the owner or an Admin can choose it again (--restart-stopped), and should ask that address first".into());
        } else if r("air_gap") || r("receive_only") {
            out.push("  it resumes by itself when that changes".into());
        } else if r("box_brake") {
            out.push(format!(
                "  the owner or an Admin resumes it ({CLI_NAME} hostmail forward resume)"
            ));
        } else if !r("manual") {
            out.push("  the owner, an Admin or the mailbox's person resumes it".into());
        }
    }
    if p::truthy(g(f, "cantArrive", line::DETAIL_CANT, F)?.as_ref()) {
        let w = idx(f, "cantArrive", line::DETAIL_CANT + 1, F)?;
        out.push(format!("  {}", s(&w)));
    }
    if p::truthy(g(f, "noticeWords", line::DETAIL_NOTICE_WORDS, F)?.as_ref()) {
        let w = idx(f, "noticeWords", line::DETAIL_NOTICE_WORDS + 1, F)?;
        out.push(format!("  {}", s(&w)));
    } else if is(&g(f, "notice", line::DETAIL_NOTICE, F)?, "sent")
        && p::truthy(g(f, "noticeAt", line::DETAIL_NOTICE, F)?.as_ref())
    {
        let l = line::DETAIL_TOLD;
        let to = or(g(f, "to", l, F)?, sv("the address"));
        let at = idx(f, "noticeAt", l, F)?;
        out.push(format!(
            "  told {}: emailed it a stop link {}",
            s(&to),
            when(Some(&at))
        ));
    }
    let c = or(
        g(f, "last24h", line::DETAIL_C, F)?,
        Value::Object(Vec::new()),
    );
    let mut show = is(&g(f, "kind", line::DETAIL_ANY, F)?, "external");
    if !show {
        for k in ["forwarded", "delivered", "refused"] {
            let v = attr_get(&c, k)
                .at(line::DETAIL_ANY, GENEXPR)
                .within(line::DETAIL_ANY, F)?;
            if p::truthy(v.as_ref()) {
                show = true;
                break;
            }
        }
    }
    if show {
        let l = line::DETAIL_COUNTS;
        let zero = || Value::Int("0".into());
        let fw = p::get_or(&c, "forwarded", zero()).at(l, F)?;
        let de = p::get_or(&c, "delivered", zero()).at(l, F)?;
        let re = p::get_or(&c, "refused", zero()).at(l, F)?;
        out.push(format!(
            "  last 24h: forwarded {}, delivered {}, refused {}",
            s(&fw),
            s(&de),
            s(&re)
        ));
    }
    if p::truthy(g(f, "setBy", line::DETAIL_SET_BY, F)?.as_ref()) {
        let w = idx(f, "setBy", line::DETAIL_SET_BY + 1, F)?;
        out.push(format!("  set by {}", s(&w)));
    }
    Ok(out)
}
