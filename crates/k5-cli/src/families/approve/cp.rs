//! The steps every `_compute_call` family of this lane shares (approve,
//! send-feedback, notifications, notify and clone propose), as the shell
//! CLI runs them (`cli/src/compute-lib.sh`, the end of
//! `cli/src/cli-tools-policy.sh`). It lives in a family's module until the
//! gardener gives it a place of its own.
//!
//! - [`open`]: the settings, the two gates and the client of an open tool
//!   (no passport check after the gates). A line the shell counts as one
//!   that needs no daemon (`_skip_conn_gate`) still meets the virtual
//!   desktop's guard, as the shell's does.
//! - [`call`]: `_compute_call` with `_K2_REFUSAL_AREA` empty: the App
//!   transport's send ([`Ctx::send`]), and any status but 200 is
//!   `_compute_fail` with no refusal registry ([`fail`]).
//! - [`utc_time`]: `time.strftime(fmt, time.gmtime(v))`.

use super::super::{curl_port, without_nuls, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::app::call::{fail_exit, Ctx, Split};
use crate::families::app::py::{self as p, Out};
use crate::out;
use crate::out::json::Value;
use crate::out::py;
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use std::ffi::OsString;
use std::path::PathBuf;

/// What [`open`] found: the run's [`Ctx`], or the end of the command.
pub enum Opened {
    /// The gates passed; the client is ready, and the K2 home the
    /// settings came from.
    Ready(Box<Ctx>, std::path::PathBuf),
    /// A gate stopped it (its lines printed), or the settings failed.
    Stop(Exit),
    /// The shell's: a `python3` the binary can't tell, or a port curl
    /// won't take.
    Shell,
}

/// The settings, the gates and the client of an open tool. `local`: the
/// line needs no daemon (`_skip_conn_gate=1`), so only the virtual
/// desktop's guard stops it, and [`Opened::Ready`] is never given: the
/// caller answers it with [`Opened::Stop`]`(Exit::Ok)` meaning go on
/// without a client.
pub fn open(args: &[OsString], local: bool) -> Opened {
    if !python::family_native(python::here()) {
        return Opened::Shell;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Opened::Stop(Exit::Error);
        }
    };
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) => return Opened::Stop(Gate::VirtualDesktop.stop()),
        Some(Gate::CannotConnect) if !local => {
            return Opened::Stop(Gate::CannotConnect.stop());
        }
        _ => {}
    }
    if local {
        return Opened::Stop(Exit::Ok);
    }
    let Some(port) = settings.port_text.as_deref().and_then(curl_port) else {
        return Opened::Shell;
    };
    settings.port = Some(port);
    ready(args, &settings)
}

/// The client for `settings`, whose port is set.
fn ready(args: &[OsString], settings: &Settings) -> Opened {
    let would_refuse = passport::current(args).would_refuse();
    match settings.client(would_refuse) {
        Ok(c) => Opened::Ready(Box::new(Ctx::new(c, false)), settings.home.clone()),
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            Opened::Stop(Exit::Error)
        }
    }
}

/// The EXIT trap's lines, as the command ends with `exit`; the exit.
pub fn close(mut ctx: Box<Ctx>, exit: Exit) -> Exit {
    let closing = std::mem::take(&mut ctx.closing);
    closing.flush(&mut ctx.client, exit);
    exit
}

/// `_compute_call <GET|POST> <path> <payload>` with no refusal area: the
/// body of a 200, else `_compute_fail`'s line and exit.
pub fn call(ctx: &mut Ctx, post: bool, path: &str, payload: &str) -> Result<Vec<u8>, Exit> {
    let s = ctx.send(post, path, payload, false)?;
    if s.ok() {
        Ok(s.body)
    } else {
        Err(fail(&s))
    }
}

/// `_compute_call <GET|POST> <path> <payload> tcp`: TCP only.
pub fn call_tcp(ctx: &mut Ctx, post: bool, path: &str, payload: &str) -> Result<Vec<u8>, Exit> {
    let s = ctx.send(post, path, payload, true)?;
    if s.ok() {
        Ok(s.body)
    } else {
        Err(fail(&s))
    }
}

/// `_app_rt_post <path> <body>`: the POST, its status and body, no
/// check.
pub fn rt_post(ctx: &mut Ctx, path: &str, body: &str) -> Result<Split, Exit> {
    ctx.send(true, path, body, false)
}

/// `_app_rt_post`, then `_compute_fail` unless the status is one of `ok`.
pub fn rt_post_ok(ctx: &mut Ctx, path: &str, body: &str, ok: &[&str]) -> Result<Vec<u8>, Exit> {
    let s = rt_post(ctx, path, body)?;
    if ok.contains(&s.status.as_str()) {
        Ok(s.body)
    } else {
        Err(fail(&s))
    }
}

/// `_email_post <path> <body>` (`cli/src/app-auth.sh`): the POST; a 403
/// `vault_confirm_required` outside a session asks for a confirm token
/// (at most three times) and sends the body again with it; then any
/// status but 200 or 202 is `_compute_fail`. `home` is the K2 home whose
/// `vault.sock` the confirm asks.
pub fn email_post(
    ctx: &mut Ctx,
    home: &std::path::Path,
    path: &str,
    body: &str,
) -> Result<Vec<u8>, Exit> {
    let mut body = body.to_string();
    let mut tries = 0;
    loop {
        let s = rt_post(ctx, path, &body)?;
        let wants = s.status == "403"
            && s
                .body
                .windows(24)
                .any(|w| w == b"\"vault_confirm_required\"")
            && ctx.client.passport().is_none()
            && tries < 3;
        if !wants {
            if s.status == "200" || s.status == "202" {
                return Ok(s.body);
            }
            return Err(fail(&s));
        }
        let error = |line: u32| -> Result<Value, Exit> {
            let v = p::load(&s.body, line).map_err(|r| raised(&r))?;
            let e = p::dget(&v, "error", line).map_err(|r| raised(&r))?;
            Ok(p::or(e, Value::Object(vec![])))
        };
        let e = error(1)?;
        let action = p::or(p::dget(&e, "action", 1).map_err(|r| raised(&r))?, p::text(""));
        let action = held(&action);
        let e = error(1)?;
        let mut sid = p::dget(&e, "secretId", 1).map_err(|r| raised(&r))?;
        if !p::truthy(&sid) {
            sid = p::or(p::dget(&e, "serviceId", 1).map_err(|r| raised(&r))?, p::text(""));
        }
        let sid = held(&sid);
        if action.is_empty() || sid.is_empty() {
            return Err(fail(&s));
        }
        let sock = crate::confirm::sock_path(home);
        let passport = ctx.client.passport().is_some();
        let mut confirm = crate::confirm::Confirm {
            client: &mut ctx.client,
            sock: &sock,
            closing: &mut ctx.closing,
            passport,
        };
        let Some(tok) = crate::confirm::confirm_token(&mut confirm, &action, &sid, "") else {
            return Err(Exit::Refused);
        };
        let field = if action == "app-email-any" {
            "confirmAnyToken"
        } else {
            "confirmToken"
        };
        body = crate::confirm::with_confirm_token(&body, &tok, Some(field)).ok_or(Exit::Error)?;
        tries += 1;
    }
}

/// `_compute_fail "$_CP_STATUS" "$_CP_BODY"` with `_K2_REFUSAL_AREA`
/// empty: one line on stderr, exit 2 for a bad request, else 3. A body
/// that is not UTF-8 makes the shell's Python fail to read it, so the line
/// is the no-answer one.
pub fn fail(s: &Split) -> Exit {
    let body: &[u8] = if std::str::from_utf8(&s.body).is_ok() {
        &s.body
    } else {
        b""
    };
    out::stderr_line(&crate::spec::run::fail_line(&s.status, body));
    fail_exit(&s.status)
}

/// What a `$(python3 -c 'print(...)')` printed of `v`: `str(v)`, its
/// trailing newlines dropped.
pub fn held(v: &Value) -> String {
    let mut s = py::str_of(v);
    while s.ends_with('\n') {
        s.pop();
    }
    s
}

/// A step's Python output: printed now, and the exit when it raised.
pub fn show(s: &Shown) -> Result<(), Exit> {
    let exit = s.print();
    if exit == Exit::Ok {
        Ok(())
    } else {
        Err(exit)
    }
}

/// `$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys;
/// print(json.load(sys.stdin)<what>)')`: one line of Python.
pub fn read_one(body: &[u8], key: &str, get: bool) -> Result<String, Raise> {
    let v = p::load(body, 1)?;
    let x = if get {
        p::dget_or(&v, key, p::text(""), 1)?
    } else {
        p::idx(&v, key, 1)?
    };
    Ok(held(&x))
}

/// A one-line Python step that raised: its traceback, exit 1.
pub fn raised(r: &Raise) -> Exit {
    Shown::err(r.text(), Exit::Error).print()
}

/// `printf '%s\n' "$_CP_BODY"`.
pub fn body_line(body: &[u8]) -> Exit {
    let mut b = without_nuls(body);
    b.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &b) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `echo "<line>"`.
pub fn stdout_line(line: &str) -> Exit {
    match out::stdout_line(line) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// One run: the request context and the K2 home.
pub struct Run {
    pub ctx: Box<Ctx>,
    pub home: PathBuf,
}

/// The settings, the gates and the client; `None` from `Err`: the shell's.
pub fn start(args: &[OsString]) -> Result<Run, Option<Exit>> {
    match cp::open(args, false) {
        Opened::Shell => Err(None),
        Opened::Stop(e) => Err(Some(e)),
        Opened::Ready(ctx, home) => Ok(Run { ctx, home }),
    }
}

/// A help page on stdout.
pub fn page(key: &str) -> Exit {
    let text = crate::text::help(key).expect("the help page is embedded");
    match out::stdout(text) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// A usage line on stderr, exit 2.
pub fn usage(line: &str) -> Exit {
    out::stderr_line(line);
    Exit::Usage
}

/// `printf '%s' "$_CP_BODY" | python3 -c '<one line>'`: `f` makes the
/// line from the answer, Python's line 1.
pub fn one_line(body: &[u8], f: impl FnOnce(&Value) -> Result<String, Raise>) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 1)?;
        o.line(&f(&v)?);
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// A value Python read from the environment: UTF-8, each byte that isn't
/// as `\udcXX` (surrogateescape).
pub fn env_text(bytes: &[u8]) -> Value {
    let mut lit = String::from("\"");
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                push_escaped(&mut lit, s);
                break;
            }
            Err(e) => {
                let (good, bad) = rest.split_at(e.valid_up_to());
                push_escaped(&mut lit, std::str::from_utf8(good).unwrap_or(""));
                let n = e.error_len().unwrap_or(bad.len());
                for b in &bad[..n] {
                    lit.push_str(&format!("\\udc{b:02x}"));
                }
                rest = &bad[n..];
            }
        }
    }
    lit.push('"');
    crate::out::json::parse(&lit).unwrap_or_else(|_| p::text(&String::from_utf8_lossy(bytes)))
}

fn push_escaped(lit: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => lit.push_str("\\\""),
            '\\' => lit.push_str("\\\\"),
            c if u32::from(c) < 0x20 => lit.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => lit.push(c),
        }
    }
}

/// `time.strftime(fmt, time.gmtime(v))` for the `%Y`, `%m`, `%d`, `%H`,
/// `%M` and `%S` fields, or the exception at `line`.
pub fn utc_time(v: &Value, fmt: &str, line: u32) -> Result<String, Raise> {
    const RANGE: &str = "OverflowError: timestamp out of range for platform time_t";
    let secs: i64 = match v {
        Value::Null => crate::families::app::py::now().floor() as i64,
        Value::Bool(b) => i64::from(*b),
        Value::Int(digits) => digits.parse().map_err(|_| Raise::at(line, RANGE))?,
        Value::Float(x) if x.is_nan() => {
            return Err(Raise::at(
                line,
                "ValueError: Invalid value NaN (not a number)",
            ))
        }
        Value::Float(x) => {
            let w = x.floor();
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&w) {
                return Err(Raise::at(line, RANGE));
            }
            w as i64
        }
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: '{}' object cannot be interpreted as an integer",
                    py::type_name(other)
                ),
            ))
        }
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&y.to_string()),
            Some('m') => out.push_str(&format!("{m:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{:02}", rem / 3600)),
            Some('M') => out.push_str(&format!("{:02}", rem / 60 % 60)),
            Some('S') => out.push_str(&format!("{:02}", rem % 60)),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    Ok(out)
}

/// The proleptic Gregorian date of a day count from 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::Value;

    #[test]
    fn utc_as_python_s_gmtime() {
        let int = |n: &str| Value::Int(n.into());
        assert_eq!(
            utc_time(&int("0"), "%H:%M:%S UTC", 1).unwrap(),
            "00:00:00 UTC"
        );
        assert_eq!(
            utc_time(&int("1763600000"), "%Y-%m-%d %H:%M:%S", 1).unwrap(),
            "2025-11-20 00:53:20"
        );
        assert_eq!(
            utc_time(&Value::Float(-1.5), "%Y-%m-%d %H:%M:%S", 1).unwrap(),
            "1969-12-31 23:59:58"
        );
        assert_eq!(
            utc_time(&int("951782400"), "%Y-%m-%d", 1).unwrap(),
            "2000-02-29"
        );
        let e = utc_time(&Value::Str("x".into()), "%H", 4).unwrap_err();
        assert_eq!(
            e.exception,
            "TypeError: 'str' object cannot be interpreted as an integer"
        );
    }
}
