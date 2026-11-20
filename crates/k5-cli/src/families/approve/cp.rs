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

use super::super::{curl_port, Raise};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::app::call::{fail_exit, Ctx, Split};
use crate::out;
use crate::out::json::Value;
use crate::out::py;
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use std::ffi::OsString;

/// What [`open`] found: the run's [`Ctx`], or the end of the command.
pub enum Opened {
    /// The gates passed; the client is ready.
    Ready(Box<Ctx>),
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
        Ok(c) => Opened::Ready(Box::new(Ctx::new(c, false))),
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
