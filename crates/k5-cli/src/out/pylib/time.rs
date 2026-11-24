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
        Value::Float(f) if f.is_nan() => Err("ValueError: Invalid value NaN (not a number)".into()),
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

/// `time.strftime(fmt, time.localtime(secs))` on Windows, by the C
/// library Python uses there (the UCRT's `_localtime64_s` and
/// `strftime`, [`crt`]); `OSError` where `_localtime64_s` refuses the
/// time (before 1970 or after 3000).
#[cfg(windows)]
pub fn strftime_local(secs: i64, fmt: &str) -> Result<String, String> {
    let tm = crt::local(secs).map_err(crt::os_error)?;
    crt::format(&tm, fmt)
}

#[cfg(not(any(unix, windows)))]
pub fn strftime_local(_secs: i64, _fmt: &str) -> Result<String, String> {
    Err("OSError: no local time here".into())
}

/// The UCRT's local time, as Python on Windows reads it: `_tzset` (so
/// `TZ` counts as there), `_localtime64_s`, and `strftime` with only the
/// conversions the UCRT takes (any other would end the process in its
/// invalid-parameter handler, so it is refused first).
#[cfg(windows)]
pub mod crt {
    /// The C `struct tm` (the UCRT's has these nine fields only).
    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct Tm {
        pub tm_sec: i32,
        pub tm_min: i32,
        pub tm_hour: i32,
        pub tm_mday: i32,
        pub tm_mon: i32,
        pub tm_year: i32,
        pub tm_wday: i32,
        pub tm_yday: i32,
        pub tm_isdst: i32,
    }

    extern "C" {
        fn _tzset();
        fn _localtime64_s(tm: *mut Tm, time: *const i64) -> i32;
        fn strftime(buf: *mut u8, max: usize, fmt: *const u8, tm: *const Tm) -> usize;
        fn _get_timezone(seconds: *mut i32) -> i32;
        fn _get_dstbias(seconds: *mut i32) -> i32;
        fn _get_tzname(len: *mut usize, buf: *mut u8, size: usize, index: i32) -> i32;
        fn _mktime64(tm: *mut Tm) -> i64;
    }

    /// `mktime(tm)`: the local wall clock in `tm` (fields normalised in
    /// place, `tm_isdst` -1 to let it tell) as seconds since 1970, or -1.
    pub fn mktime(tm: &mut Tm) -> i64 {
        // SAFETY: _tzset reads TZ; _mktime64 reads and normalises `tm`,
        // which outlives the call.
        unsafe {
            _tzset();
            _mktime64(tm)
        }
    }

    /// `secs` as local time, or the errno `_localtime64_s` gave.
    pub fn local(secs: i64) -> Result<Tm, i32> {
        let mut tm = Tm::default();
        // SAFETY: _tzset reads TZ and the system's zone; _localtime64_s
        // writes only into `tm`, which outlives the call.
        let rc = unsafe {
            _tzset();
            _localtime64_s(&mut tm, &secs)
        };
        if rc == 0 {
            Ok(tm)
        } else {
            Err(rc)
        }
    }

    /// The `OSError` line for an errno of the C library's.
    pub fn os_error(errno: i32) -> String {
        format!(
            "OSError: [Errno {errno}] {}",
            crate::out::pylib::json::c_strerror(errno)
        )
    }

    /// The conversions the UCRT's `strftime` takes (`#` may lead one).
    const TAKES: &[u8] = b"aAbBcCdDeFgGhHIjmMnprRStTuUVwWxXyYzZ%";

    /// `strftime(fmt, tm)`, or the `ValueError` for a format the UCRT
    /// would not take.
    pub fn format(tm: &Tm, fmt: &str) -> Result<String, String> {
        let b = fmt.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                i += 1;
                if b.get(i) == Some(&b'#') {
                    i += 1;
                }
                if !b.get(i).is_some_and(|c| TAKES.contains(c)) {
                    return Err("ValueError: Invalid format string".into());
                }
            }
            i += 1;
        }
        if b.contains(&0) {
            return Err("ValueError: embedded null character".into());
        }
        let mut cfmt = b.to_vec();
        cfmt.push(0);
        let mut buf = vec![0u8; 256 + 4 * b.len()];
        // SAFETY: `buf` and `cfmt` live through the call, the size is
        // `buf`'s, `cfmt` ends in a NUL and holds only conversions the
        // UCRT takes (checked above).
        let n = unsafe { strftime(buf.as_mut_ptr(), buf.len(), cfmt.as_ptr(), tm) };
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    /// The UTC offset of `tm` in seconds east, and its zone's name:
    /// `tm_gmtoff` and `tm_zone` where Unix has them.
    pub fn offset_and_zone(tm: &Tm) -> (i64, String) {
        let (mut zone, mut bias) = (0i32, 0i32);
        let mut name = [0u8; 128];
        let mut len = 0usize;
        let dst = tm.tm_isdst > 0;
        // SAFETY: each call writes only into the local it is given, `name`
        // with its own size.
        unsafe {
            _get_timezone(&mut zone);
            _get_dstbias(&mut bias);
            if _get_tzname(&mut len, name.as_mut_ptr(), name.len(), i32::from(dst)) != 0 {
                len = 0;
            }
        }
        let west = i64::from(zone) + if dst { i64::from(bias) } else { 0 };
        let text = &name[..len.saturating_sub(1).min(name.len())];
        (-west, String::from_utf8_lossy(text).into_owned())
    }
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// The UCRT's local time: a year near 1970 for 0, the fields of a
    /// real time, a refused time's errno, and a format it would not take
    /// refused before it is called.
    #[test]
    fn the_ucrt_reads_the_local_time() {
        let y = strftime_local(0, "%Y").unwrap();
        assert!(y == "1970" || y == "1969", "{y}");
        let t = strftime_local(1_700_000_000, "%Y-%m-%d %H:%M").unwrap();
        assert_eq!(t.len(), 16, "{t}");
        assert!(t.starts_with("2023-11-1"), "{t}");
        assert_eq!(
            strftime_local(-100_000, "%Y"),
            Err("OSError: [Errno 22] Invalid argument".to_string())
        );
        assert_eq!(
            strftime_local(0, "%-d"),
            Err("ValueError: Invalid format string".to_string())
        );
        assert_eq!(strftime_local(0, "100%%").unwrap(), "100%");
        let tm = crt::local(1_700_000_000).unwrap();
        let (off, _zone) = crt::offset_and_zone(&tm);
        assert!(off.abs() <= 14 * 3600, "{off}");
    }
}
