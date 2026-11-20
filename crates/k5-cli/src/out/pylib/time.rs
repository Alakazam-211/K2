//! Python's `time.strftime(fmt, time.localtime(secs))`, as the shell's
//! readers print a time: the C library's `localtime_r` and `strftime`, as
//! Python's are, with the same `TZ`. First written for activity, then
//! copied by usage; shared here.

use crate::out::json::Value;
use crate::out::py;

/// What `time.localtime` says about a time past `time_t`.
pub const RANGE: &str = "OverflowError: timestamp out of range for platform time_t";

/// The whole seconds `time.localtime(v)` reads from a JSON value, or the
/// exception's line. `now`: what `None` stands for.
pub fn seconds(v: &Value, now: i64) -> Result<i64, String> {
    match v {
        Value::Null => Ok(now),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Int(digits) => digits.parse().map_err(|_| RANGE.to_string()),
        Value::Float(f) if f.is_nan() => {
            Err("ValueError: Invalid value NaN (not a number)".into())
        }
        Value::Float(f) => {
            let w = f.floor();
            // i64::MAX as f64 rounds up to 2^63, which is out of range.
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&w) {
                return Err(RANGE.into());
            }
            Ok(w as i64)
        }
        other => Err(format!(
            "TypeError: '{}' object cannot be interpreted as an integer",
            py::type_name(other)
        )),
    }
}

/// `time.strftime(fmt, time.localtime(secs))`, by the C library's
/// `localtime_r` and `strftime`; `OSError` where `localtime_r` fails.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn strftime_local(secs: i64, fmt: &str) -> Result<String, String> {
    // `time_t` is 64 bits on every platform the binary ships for.
    let t = secs as libc::time_t;
    let mut cfmt = fmt.as_bytes().to_vec();
    cfmt.push(0);
    // SAFETY: tzset reads TZ; localtime_r and strftime write only into the
    // buffers given, each sized as passed; the format ends with a NUL.
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
        let mut buf = [0u8; 64];
        let n = libc::strftime(
            buf.as_mut_ptr().cast(),
            buf.len(),
            cfmt.as_ptr().cast(),
            &tm,
        );
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

#[cfg(not(unix))]
pub fn strftime_local(_secs: i64, _fmt: &str) -> Result<String, String> {
    Err("OSError: no local time here".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::Text;

    #[test]
    fn the_seconds_python_reads() {
        assert_eq!(seconds(&Value::Null, 7), Ok(7));
        assert_eq!(seconds(&Value::Bool(true), 0), Ok(1));
        assert_eq!(seconds(&Value::Int("-5".into()), 0), Ok(-5));
        assert_eq!(seconds(&Value::Float(1.9), 0), Ok(1));
        assert_eq!(seconds(&Value::Float(-1.5), 0), Ok(-2));
        assert_eq!(
            seconds(&Value::Int("99999999999999999999".into()), 0),
            Err(RANGE.into())
        );
        assert_eq!(seconds(&Value::Float(9.3e18), 0), Err(RANGE.into()));
        assert_eq!(
            seconds(&Value::Float(f64::NAN), 0),
            Err("ValueError: Invalid value NaN (not a number)".into())
        );
        assert_eq!(
            seconds(&Value::Str(Text::from("1")), 0),
            Err("TypeError: 'str' object cannot be interpreted as an integer".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_c_library_s_strftime() {
        let a = strftime_local(86_400, "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(a.len(), 19, "{a}");
        assert_eq!(strftime_local(86_400, "x%%y").as_deref(), Ok("x%y"));
        assert_eq!(strftime_local(86_400, "").as_deref(), Ok(""));
    }
}
