//! `tunnel status [--json]` and `tunnel subdomains [--json]`: one
//! `raw=$(cli_request …)` (a failed call ends the command with its code;
//! one with no answer prints the transport's line, exit 1), printed as it
//! came with `--json`, else read by the shell's Python. `subdomains`
//! first asks for the workspaces' names (`/cli/companion/projects`, its
//! errors hidden; a failed call still ends the command with its code).

use super::super::{get, ljust, py_get, py_iter, py_len, py_space, Raise, Shown};
use super::{is, load, or, plus, truthy, unread, Out};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::{Call, Channel, Client, Error};
use k2_client::hosts::{RELAY_HOST, ZONE, ZONE_SUFFIX};
use k2_client::{cli_name, product_name};

/// `raw=$(cli_request <endpoint>)`: the body as `$(…)` holds it, or the
/// command's exit. `quiet`: its stderr goes to /dev/null.
fn request(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    quiet: bool,
) -> Result<Vec<u8>, Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::Request(&[])) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(super::held(&r))
        }
        Err(Error::NoAnswer(n)) => {
            if !quiet {
                out::stderr_line(&n.to_string());
            }
            Err(Exit::Error)
        }
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// `printf '%s\n' "$raw"`.
fn as_is(raw: &[u8]) -> Exit {
    let mut bytes = raw.to_vec();
    bytes.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &bytes) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `tunnel status [--json]`.
pub fn status(client: &mut Client, closing: &mut Closing, json: bool) -> Exit {
    let raw = match request(client, closing, super::STATUS, false) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    if json {
        return as_is(&raw);
    }
    status_reader(&raw).print()
}

/// `tunnel subdomains [--json]`.
pub fn subdomains(client: &mut Client, closing: &mut Closing, json: bool) -> Exit {
    let raw = match request(client, closing, super::SUBDOMAINS, false) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    if json {
        return as_is(&raw);
    }
    let projects = match request(client, closing, super::PROJECTS, true) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    subdomains_reader(&raw, &projects).print()
}

/// `x` when `bool(x)`.
fn truthy_v(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| py::truthy(x))
}

/// `x or {}`.
fn or_dict(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Object(Vec::new()),
    }
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `a + b` where `b` is a string: `a` must be one too.
fn add(a: &Value, b: &str, line: u32) -> Result<Value, Raise> {
    match a {
        Value::Str(_) => Ok(s(&format!("{}{b}", py::str_of(a)))),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `", ".join(v)`.
fn join(v: &Value, line: u32) -> Result<String, Raise> {
    let items = match v {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => py_iter(v, line)?,
        _ => {
            return Err(Raise::at(line, "TypeError: can only join an iterable"));
        }
    };
    let mut parts = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => parts.push(py::str_of(item)),
            other => {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(parts.join(", "))
}

/// `"x".strip()` on `d.get(key) or ""`.
fn stripped(v: Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(&v).trim_matches(py_space).to_string()),
        other => Err(Raise::no_attribute(line, &other, "strip")),
    }
}

/// The lines of the status reader that can raise, as `python3 -c` numbers
/// them (`tests` pins each).
pub mod line {
    pub const RUNNING: u32 = 8;
    pub const LAST_ERROR: u32 = 15;
    pub const CONFIG: u32 = 17;
    pub const TOKEN: u32 = 43;
    pub const VI_WORDS: u32 = 49;
    pub const WORDS: u32 = 50;
    pub const VI_RELAY: u32 = 51;
    pub const SERVER_PORT: u32 = 60;
    pub const PRIMARY: u32 = 62;
    pub const FALLBACK: u32 = 64;
    pub const DETAIL: u32 = 71;
    pub const RELAYS: u32 = 77;
    pub const REPLACED: u32 = 82;
    pub const BY: u32 = 85;
    pub const WHAT: u32 = 86;
    pub const CONFIG_FILE: u32 = 89;
    pub const CHECKED: u32 = 91;
    pub const ACTIVE: u32 = 95;
    pub const FAILED: u32 = 100;
    pub const NOTICE: u32 = 104;
    pub const VOICE_WHAT: u32 = 110;
    pub const TURN: u32 = 114;
    pub const VIA: u32 = 116;
    pub const ADOPTED: u32 = 118;
    pub const VOICE: u32 = 119;
    pub const VOICE_ERROR: u32 = 121;
    pub const MEDIA_GET: u32 = 123;
    pub const MEDIA: u32 = 124;
    pub const LI_SUB: u32 = 131;
    pub const LI_PORT: u32 = 132;
    pub const WANT: u32 = 134;
}

/// The voice lane's states, as the reader words them.
const VOICE_STATES: [(&str, &str); 5] = [
    ("off", "off"),
    (
        "waiting_for_tunnel",
        "armed, waiting for the tunnel (E2E, a confirmed label)",
    ),
    (
        "starting",
        "starting (frps has not accepted its turn proxy yet)",
    ),
    ("up", "up"),
    ("restarting", "restarting (frpc-voice exited)"),
];

/// `tunnel status`'s reader.
pub fn status_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse tunnel status: ", &e),
    };
    let mut o = Out::default();
    o.out(&format!("{} Connect tunnel", product_name!()));
    let r = status_lines(&d, &mut o);
    o.done(r)
}

fn status_lines(d: &Value, o: &mut Out) -> Result<Exit, Raise> {
    let running = truthy(py_get(d, "running", line::RUNNING)?);
    let Value::Object(p) = d else {
        unreachable!("d.get passed")
    };
    let g = |k: &str| get(p, k);
    if running && is(g("frpc_healthy"), false) {
        let n = match truthy_v(g("frpc_consecutive_failures")) {
            Some(v) => py::str_of(v),
            None => "0".into(),
        };
        o.out(&format!(
            "  Running:    yes, but frpc is FAILING ({n} consecutive failures)"
        ));
        if let Some(e) = truthy_v(g("frpc_last_error")) {
            o.out(&plus("  Last error: ", e, line::LAST_ERROR)?);
        }
        if is(g("frpc_config_present"), false) {
            o.out(&plus(
                "  Config:     MISSING ",
                &or(g("frpc_config"), ""),
                line::CONFIG,
            )?);
        }
        o.out(&format!("  Repair:     {} tunnel start", cli_name!()));
    } else {
        o.out(&format!(
            "  Running:    {}",
            if running { "yes" } else { "no" }
        ));
    }
    if let Some(li) = truthy_v(g("loginIngress")) {
        o.out(&format!("  Login ingress: {}", py::str_of(li)));
    }
    if let Some(bs) = truthy_v(g("bootStatus")) {
        let note = if truthy(g("bootStatusExplicit")) {
            ""
        } else {
            " (default)"
        };
        o.out(&format!("  Boot status: {}{note}", py::str_of(bs)));
    }
    if !running {
        if truthy(g("released")) {
            o.out("  State:      RELEASED \u{2014} this device is unpaired (re-pair to mint a fresh identity)");
        } else if is(g("enabled"), false) {
            o.out(&format!(
                "  State:      DISABLED (by you, persisted) \u{2014} re-enable with: {} tunnel enable",
                cli_name!()
            ));
        }
        if let Some(t) = truthy_v(g("token_locked")) {
            o.out(&plus("  Token:      LOCKED \u{2014} ", t, line::TOKEN)?);
        }
    }
    if running {
        let url = or(g("public_url"), "(assigned by server)");
        o.out(&format!("  Public URL: {}", py::str_of(&url)));
        let vi = or_dict(g("visitorIps"));
        if let Some(words) = truthy_v(py_get(&vi, "words", line::VI_WORDS)?) {
            o.out(&plus("  ", words, line::WORDS)?);
            if let Some(n) = truthy_v(py_get(&vi, "relayAsClient", line::VI_RELAY)?) {
                o.out(&format!(
                    "    a relay named its own address {} times (never counted as a visitor)",
                    py::str_of(n)
                ));
            }
        }
        if let Some(lp) = truthy_v(g("local_port")) {
            o.out(&format!("  Local port: {}", py::str_of(lp)));
        }
        if let Some(sa) = truthy_v(g("server_addr")) {
            let mut sa = sa.clone();
            if let Some(port) = truthy_v(g("server_port")) {
                sa = add(&sa, &format!(":{}", py::str_of(port)), line::SERVER_PORT)?;
            }
            if is(g("relay_primary"), true) {
                sa = add(&sa, " (primary relay)", line::PRIMARY)?;
            } else if is(g("relay_primary"), false) {
                sa = add(
                    &sa,
                    " (fallback relay; returns to the primary once it answers again)",
                    line::FALLBACK,
                )?;
            }
            o.out(&format!("  Server:     {}", py::str_of(&sa)));
        }
        if let Some(Value::Object(fo)) = g("failover") {
            if let Some(detail) = truthy_v(get(fo, "detail")) {
                o.out(&plus("  Failover:   ", detail, line::DETAIL)?);
            }
        }
    }
    if let Some(Value::Object(rl)) = g("relay_list") {
        relay_lines(rl, o)?;
    }
    if let Some(Value::Object(vl)) = g("voiceLane") {
        voice_lines(vl, o)?;
    }
    if let Some(auto) = g("autoStart") {
        if py::truthy(auto) {
            o.out("  Auto-start: on");
        } else {
            o.out(&format!(
                "  Auto-start: off ({} tunnel enable turns it on)",
                cli_name!()
            ));
        }
    }
    if let Some(li) = g("e2eListener").filter(|v| py::truthy(v)) {
        let lsub = item(li, "subdomain", line::LI_SUB)?;
        let lport = item(li, "port", line::LI_PORT)?;
        o.out(&format!(
            "  Encrypted listener: {lsub}{ZONE_SUFFIX} (port {lport})"
        ));
        let want = stripped(or(g("subdomain"), ""), line::WANT)?;
        if running && !want.is_empty() && want.to_lowercase() != lsub.to_lowercase() {
            o.out(&format!(
                "  WARNING: the tunnel serves {want}{ZONE_SUFFIX} but the listener holds the {lsub}{ZONE_SUFFIX} certificate; run: {} tunnel switch {want}",
                cli_name!()
            ));
        }
    }
    Ok(Exit::Ok)
}

/// `str(li.get(key, ""))` on a value Python must find a dict.
fn item(li: &Value, key: &str, line: u32) -> Result<String, Raise> {
    Ok(py_get(li, key, line)?.map_or_else(String::new, py::str_of))
}

fn relay_lines(rl: &[(Text, Value)], o: &mut Out) -> Result<(), Raise> {
    let r = |k: &str| get(rl, k);
    let relays = match truthy_v(r("relays")) {
        Some(v) => join(v, line::RELAYS)?,
        None => String::new(),
    };
    let relays = if relays.is_empty() {
        "(none)".to_string()
    } else {
        relays
    };
    let mut src = or(r("source"), "?");
    if let Some(rep) = truthy_v(r("replaced")) {
        let joined = join(rep, line::REPLACED)?;
        src = add(
            &src,
            &format!("; replaces the old built-in {joined}"),
            line::REPLACED,
        )?;
    }
    o.out(&format!("  Relays:     {relays} [{}]", py::str_of(&src)));
    if let Some(rd) = truthy_v(r("relay_default")) {
        let in_file = plus(
            "relay_default in ",
            &or(r("config_file"), "tunnel.json"),
            line::BY,
        )?;
        let by = match r("relay_default_set_by") {
            Some(v @ (Value::Array(_) | Value::Object(_))) => {
                return Err(Raise::at(
                    line::BY,
                    format!("TypeError: unhashable type: '{}'", py::type_name(v)),
                ))
            }
            Some(v) if *v == s("env") => {
                "K2_RELAY_DEFAULT in the daemon environment, which wins over tunnel.json"
                    .to_string()
            }
            Some(v) if *v == s("tunnel.json") => in_file,
            _ => "tunnel.json relay_default or K2_RELAY_DEFAULT".to_string(),
        };
        let what = if *rd == s("legacy") {
            "legacy (pre-0.46.6, the old built-in IP".to_string()
        } else {
            py::str_of(&add(
                rd,
                &format!(" ({RELAY_HOST}, the 0.46.6 default"),
                line::WHAT,
            )?)
        };
        o.out(&format!("  Relay default: {what}; set by {by})"));
    }
    if let Some(cf) = truthy_v(r("config_file")) {
        o.out(&plus("  Tunnel config: ", cf, line::CONFIG_FILE)?);
    }
    if let Some(at) = truthy_v(r("checked_at")) {
        let ts = from_timestamp(at).map_err(|e| Raise::at(line::CHECKED, e))?;
        o.out(&format!("  Relay list: checked with {ZONE} {ts}"));
    }
    if let Some(a) = truthy_v(r("active")) {
        let points = plus(
            &format!("  Live relay: *{ZONE_SUFFIX} points at "),
            a,
            line::ACTIVE,
        )?;
        o.out(&format!("{points} (per {ZONE})"));
    }
    let last = truthy_v(r("last_error"));
    if last.is_some_and(|e| py::str_of(e).starts_with("401")) {
        o.out(&format!("  Relay list: {ZONE} does not hand out a relay list for this tunnel token; the saved list stays in use"));
    } else if let Some(e) = last {
        o.out(&plus(
            "  Relay list: last refresh failed: ",
            e,
            line::FAILED,
        )?);
    } else if let Some(n) = truthy_v(r("notice")) {
        o.out(&plus("  Relay list: ", n, line::NOTICE)?);
    }
    Ok(())
}

fn voice_lines(vl: &[(Text, Value)], o: &mut Out) -> Result<(), Raise> {
    let v = |k: &str| get(vl, k);
    let st = or(v("state"), "?");
    let mut what = match &st {
        Value::Array(_) | Value::Object(_) => {
            return Err(Raise::at(
                line::VOICE_WHAT,
                format!("TypeError: unhashable type: '{}'", py::type_name(&st)),
            ))
        }
        _ => VOICE_STATES
            .iter()
            .find(|(k, _)| st == s(k))
            .map_or_else(|| st.clone(), |(_, w)| s(w)),
    };
    if let Some(host) = truthy_v(v("turnHost")).filter(|_| st != s("off")) {
        let rhs = plus(" for ", host, line::TURN)?;
        what = add(&what, &rhs, line::TURN)?;
    }
    if let Some(addr) = truthy_v(v("serverAddr")) {
        let port = or(v("serverPort"), "");
        let rhs = plus(" via ", addr, line::VIA)?;
        what = add(&what, &format!("{rhs}:{}", py::str_of(&port)), line::VIA)?;
    }
    if truthy(v("adopted")) {
        what = add(&what, " (adopted)", line::ADOPTED)?;
    }
    o.out(&plus("  Voice lane: ", &what, line::VOICE)?);
    if let Some(e) = truthy_v(v("lastError")).filter(|_| st != s("up")) {
        o.out(&plus("  Voice lane error: ", e, line::VOICE_ERROR)?);
    }
    let mp = or_dict(v("mediaPath"));
    if let Some(state) = truthy_v(py_get(&mp, "state", line::MEDIA_GET)?) {
        if *state != s("off") {
            let head = plus("  Media path: ", state, line::MEDIA)?;
            let tail = if truthy(py_get(&mp, "sewHasRoute", line::MEDIA)?) {
                ""
            } else {
                " (this Sew has no media-path route)"
            };
            o.out(&format!("{head}{tail}"));
        }
    }
    Ok(())
}

/// `datetime.datetime.fromtimestamp(v).strftime("%Y-%m-%d %H:%M")`, or
/// the exception's line.
pub fn from_timestamp(v: &Value) -> Result<String, String> {
    const RANGE: &str = "OverflowError: timestamp out of range for platform time_t";
    let secs: i64 = match v {
        Value::Bool(b) => i64::from(*b),
        Value::Int(digits) => digits.parse().map_err(|_| RANGE.to_string())?,
        Value::Float(f) if f.is_nan() => {
            return Err("ValueError: Invalid value NaN (not a number)".into())
        }
        Value::Float(f) if f.is_infinite() => {
            return Err("OverflowError: cannot convert float infinity to integer".into())
        }
        Value::Float(f) => {
            // Microseconds rounded half to even, as Python rounds them.
            let mut whole = f.trunc();
            let micros = ((f - whole) * 1e6).round_ties_even();
            if micros >= 1e6 {
                whole += 1.0;
            } else if micros < 0.0 {
                whole -= 1.0;
            }
            if !(-9.2e18..9.2e18).contains(&whole) {
                return Err(RANGE.into());
            }
            whole as i64
        }
        other => {
            return Err(format!(
                "TypeError: '{}' object cannot be interpreted as an integer",
                py::type_name(other)
            ))
        }
    };
    local_minutes(secs)
}

/// The local time of `secs` as `%Y-%m-%d %H:%M`, by `localtime_r` and
/// `strftime`, as Python's `datetime` gets it.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn local_minutes(secs: i64) -> Result<String, String> {
    let t = secs as libc::time_t;
    // SAFETY: tzset reads TZ; localtime_r and strftime write only into the
    // buffers given, each sized as passed.
    unsafe {
        extern "C" {
            fn tzset();
        }
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            let e = std::io::Error::last_os_error();
            let n = e.raw_os_error().unwrap_or(0);
            let text = e.to_string();
            let msg = text
                .strip_suffix(&format!(" (os error {n})"))
                .unwrap_or(&text);
            return Err(format!("OSError: [Errno {n}] {msg}"));
        }
        let year = i64::from(tm.tm_year) + 1900;
        if !(1..=9999).contains(&year) {
            return Err(format!("ValueError: year {year} is out of range"));
        }
        let mut buf = [0u8; 64];
        let fmt = b"%Y-%m-%d %H:%M\0";
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr().cast(), &tm);
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

/// Off Unix, the C library there; the same year check.
#[cfg(windows)]
fn local_minutes(secs: i64) -> Result<String, String> {
    use crate::out::pylib::time::crt;
    let tm = crt::local(secs).map_err(crt::os_error)?;
    let year = i64::from(tm.tm_year) + 1900;
    if !(1..=9999).contains(&year) {
        return Err(format!("ValueError: year {year} is out of range"));
    }
    crt::format(&tm, "%Y-%m-%d %H:%M")
}

#[cfg(not(any(unix, windows)))]
fn local_minutes(_secs: i64) -> Result<String, String> {
    Err("OSError: no local time here".into())
}

/// Python's `==` for two dict keys: numbers by value (`1 == 1.0 == True`).
fn same_key(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Int(d) => d.parse::<f64>().ok(),
        Value::Float(f) => Some(*f),
        _ => None,
    };
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// The workspaces' names the subdomains reader builds from
/// `K2_PROJECTS_JSON` (`id -> name`); whatever goes wrong stops it there.
pub fn project_names(projects: &[u8]) -> Vec<(Value, Value)> {
    let text = String::from_utf8_lossy(projects);
    let text = if text.is_empty() { "[]".into() } else { text };
    let mut names: Vec<(Value, Value)> = Vec::new();
    let Ok(all) = crate::out::json::parse(&text) else {
        return names;
    };
    let Value::Array(items) = all else {
        return names;
    };
    for p in &items {
        let Value::Object(pairs) = p else { continue };
        let Some(id) = truthy_v(get(pairs, "id")) else {
            continue;
        };
        if matches!(id, Value::Array(_) | Value::Object(_)) {
            break;
        }
        let name = truthy_v(get(pairs, "name")).unwrap_or(id).clone();
        match names.iter_mut().find(|(k, _)| same_key(k, id)) {
            Some(slot) => slot.1 = name,
            None => names.push((id.clone(), name)),
        }
    }
    names
}

/// The lines of the subdomains reader that can raise.
pub mod sub_line {
    pub const ERROR: u32 = 7;
    pub const PRIMARY: u32 = 17;
    pub const SORTED: u32 = 27;
    pub const INDEX: u32 = 29;
    pub const NAMES: u32 = 35;
    pub const HW: u32 = 41;
    pub const TW: u32 = 42;
}

/// `tunnel subdomains`'s reader: `raw` on stdin, `projects` the names'
/// answer as `$(…)` held it.
pub fn subdomains_reader(raw: &[u8], projects: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse tunnel subdomains: ", &e),
    };
    let mut o = Out::default();
    let r = subdomain_lines(&d, projects, &mut o);
    o.done(r)
}

fn subdomain_lines(d: &Value, projects: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let err = py_get(d, "error", sub_line::ERROR)?;
    if let Some(e) = truthy_v(err) {
        o.err(&format!(
            "Failed to read tunnel subdomains: {}",
            py::str_of(e)
        ));
        return Ok(Exit::Error);
    }
    let Value::Object(p) = d else {
        unreachable!("d.get passed")
    };
    let names = project_names(projects);
    let primary = stripped(or(get(p, "primary"), ""), sub_line::PRIMARY)?;
    let targets = or_dict(get(p, "targets"));
    o.out(&format!(
        "{} Connect subdomains (daemon routing map)",
        product_name!()
    ));
    if primary.is_empty() && !py::truthy(&targets) {
        o.out("  (empty \u{2014} the daemon has not learned a routing map yet;");
        o.out("   requires the tunnel running with E2E on)");
        return Ok(Exit::Ok);
    }
    let mut rows: Vec<(Value, Value, Value)> = Vec::new();
    if !primary.is_empty() {
        rows.push((
            s(&format!("{primary}{ZONE_SUFFIX}")),
            s("daemon (primary)"),
            s("-"),
        ));
    }
    for (label, t) in labelled(&targets)? {
        let host = if primary.is_empty() {
            label.clone()
        } else {
            s(&format!("{}.{primary}{ZONE_SUFFIX}", py::str_of(&label)))
        };
        let (endpoint, ws) = match &t {
            Value::Object(tp) => {
                let endpoint = or(get(tp, "target"), "?");
                let ws = match truthy_v(get(tp, "projectId")) {
                    Some(pid @ (Value::Array(_) | Value::Object(_))) => {
                        return Err(Raise::at(
                            sub_line::NAMES,
                            format!("TypeError: unhashable type: '{}'", py::type_name(pid)),
                        ))
                    }
                    Some(pid) => names
                        .iter()
                        .find(|(k, _)| same_key(k, pid))
                        .map_or_else(|| pid.clone(), |(_, n)| n.clone()),
                    None => s("-"),
                };
                (endpoint, ws)
            }
            other => (s(&py::str_of(other)), s("-")),
        };
        rows.push((host, endpoint, ws));
    }
    let mut hw = "HOST".len();
    for r in &rows {
        hw = hw.max(py_len(&r.0, sub_line::HW)?);
    }
    let mut tw = "TARGET".len();
    for r in &rows {
        tw = tw.max(py_len(&r.1, sub_line::TW)?);
    }
    o.out(&format!(
        "  {}  {}  WORKSPACE",
        ljust("HOST", hw),
        ljust("TARGET", tw)
    ));
    for (host, endpoint, ws) in &rows {
        o.out(&format!(
            "  {}  {}  {}",
            ljust(&py::str_of(host), hw),
            ljust(&py::str_of(endpoint), tw),
            py::str_of(ws)
        ));
    }
    if !py::truthy(&targets) {
        o.out("  (no nested subdomains configured)");
    }
    Ok(Exit::Ok)
}

/// `for label in sorted(targets): t = targets[label]`, for the shapes the
/// daemon could send: an object's keys in order; a list of whole numbers
/// as indexes; anything else raises where Python does.
fn labelled(targets: &Value) -> Result<Vec<(Value, Value)>, Raise> {
    match targets {
        Value::Object(pairs) => {
            let mut keys: Vec<&(Text, Value)> = pairs.iter().collect();
            keys.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(keys
                .into_iter()
                .map(|(k, v)| (Value::Str(k.clone()), v.clone()))
                .collect())
        }
        Value::Array(items) => {
            let index = |v: &Value| match v {
                Value::Bool(b) => Some(i64::from(*b)),
                Value::Int(d) => d.parse::<i64>().ok(),
                _ => None,
            };
            if items.iter().all(|v| matches!(v, Value::Str(_))) {
                return Err(Raise::at(
                    sub_line::INDEX,
                    "TypeError: list indices must be integers or slices, not str",
                ));
            }
            let Some(mut ix): Option<Vec<(i64, &Value)>> =
                items.iter().map(|v| index(v).map(|i| (i, v))).collect()
            else {
                return Err(Raise::at(
                    sub_line::SORTED,
                    "TypeError: '<' not supported between instances",
                ));
            };
            // Python's sort is stable: equal numbers keep their order.
            ix.sort_by_key(|(i, _)| *i);
            let n = items.len() as i64;
            let mut out = Vec::with_capacity(ix.len());
            for (i, label) in ix {
                let at = if i < 0 { n + i } else { i };
                let Some(t) = usize::try_from(at).ok().and_then(|a| items.get(a)) else {
                    return Err(Raise::at(
                        sub_line::INDEX,
                        "IndexError: list index out of range",
                    ));
                };
                out.push((label.clone(), t.clone()));
            }
            Ok(out)
        }
        Value::Str(_) => Err(Raise::at(
            sub_line::INDEX,
            "TypeError: string indices must be integers, not 'str'",
        )),
        other => Err(Raise::at(
            sub_line::SORTED,
            format!(
                "TypeError: '{}' object is not iterable",
                py::type_name(other)
            ),
        )),
    }
}
