//! What Python 3.12's `json.loads` says about a text it can't read, and
//! how Python reads the text a command takes from a file or stdin.
//!
//! [`loads`] is [`crate::out::json::parse`] with Python's own error: the
//! message `JSONDecodeError` carries (`Expecting value: line 1 column 1
//! (char 0)`), found by walking the text as Python's C scanner does, by
//! code point. The shell's board commands print it inside their own usage
//! line (`the ops are not JSON: …`), so its words and its place must be
//! Python's.

use crate::out::json::{self, Value};

/// Why Python could not read a text as JSON: the exception's class and
/// its message, as `str(e)` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub class: &'static str,
    pub message: String,
}

impl Failed {
    /// The traceback's last line: `<module>.<class>: <message>`.
    pub fn last_line(&self) -> String {
        let module = if self.class == "JSONDecodeError" {
            "json.decoder."
        } else {
            ""
        };
        format!("{module}{}: {}", self.class, self.message)
    }
}

/// `json.loads(text)`: the value, or what Python raises.
pub fn loads(text: &str) -> Result<Value, Failed> {
    let points: Vec<char> = text.chars().collect();
    if let Some((what, at)) = first_error(&points) {
        return Err(decode_error(&points, what, at));
    }
    match json::parse(text) {
        Ok(v) => Ok(v),
        Err(json::Error::IntTooLong { at }) => {
            // The digits of the long integer at byte `at`.
            let digits = text[at..]
                .trim_start_matches('-')
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            Err(Failed {
                class: "ValueError",
                message: format!(
                    "Exceeds the limit ({} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit",
                    json::MAX_INT_DIGITS
                ),
            })
        }
        Err(json::Error::TooDeep { .. }) => Err(Failed {
            class: "RecursionError",
            message:
                "maximum recursion depth exceeded while decoding a JSON array from a unicode string"
                    .to_string(),
        }),
        // The walk above found none; a reader that disagrees says so.
        Err(e) => Err(Failed {
            class: "JSONDecodeError",
            message: e.to_string(),
        }),
    }
}

/// `JSONDecodeError(what, doc, at)`'s message.
fn decode_error(points: &[char], what: &str, at: usize) -> Failed {
    let before = &points[..at.min(points.len())];
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let column = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => at - nl,
        None => at + 1,
    };
    Failed {
        class: "JSONDecodeError",
        message: format!("{what}: line {line} column {column} (char {at})"),
    }
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn skip_ws(p: &[char], mut i: usize) -> usize {
    while p.get(i).is_some_and(|c| is_ws(*c)) {
        i += 1;
    }
    i
}

type Stop = (&'static str, usize);

/// The first error Python's decoder raises on `p`, if any.
fn first_error(p: &[char]) -> Option<Stop> {
    if p.first() == Some(&'\u{feff}') {
        return Some(("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = skip_ws(p, 0);
    match value(p, start) {
        Err(e) => Some(e),
        Ok(end) => {
            let end = skip_ws(p, end);
            (end != p.len()).then_some(("Extra data", end))
        }
    }
}

/// `scan_once` at `i`: the end of the value, or the error.
fn value(p: &[char], i: usize) -> Result<usize, Stop> {
    let expecting = ("Expecting value", i);
    let Some(&c) = p.get(i) else {
        return Err(expecting);
    };
    let word = |w: &str| {
        let n = w.chars().count();
        (p.len() >= i + n && p[i..i + n].iter().copied().eq(w.chars())).then_some(i + n)
    };
    match c {
        '"' => string(p, i + 1),
        '{' => object(p, i + 1),
        '[' => array(p, i + 1),
        'n' => word("null").ok_or(expecting),
        't' => word("true").ok_or(expecting),
        'f' => word("false").ok_or(expecting),
        'N' => word("NaN").ok_or(expecting),
        'I' => word("Infinity").ok_or(expecting),
        '-' if word("-Infinity").is_some() => Ok(i + 9),
        _ => number(p, i).ok_or(expecting),
    }
}

/// `_match_number_unicode`: the end of the number at `i`, if one starts
/// there.
fn number(p: &[char], i: usize) -> Option<usize> {
    let digit = |j: usize| p.get(j).is_some_and(char::is_ascii_digit);
    let mut j = i;
    if p.get(j) == Some(&'-') {
        j += 1;
    }
    match p.get(j) {
        Some('1'..='9') => {
            j += 1;
            while digit(j) {
                j += 1;
            }
        }
        Some('0') => j += 1,
        _ => return None,
    }
    if p.get(j) == Some(&'.') && digit(j + 1) {
        j += 2;
        while digit(j) {
            j += 1;
        }
    }
    if matches!(p.get(j), Some('e' | 'E')) {
        let mut k = j + 1;
        if matches!(p.get(k), Some('+' | '-')) {
            k += 1;
        }
        if digit(k) {
            while digit(k) {
                k += 1;
            }
            j = k;
        }
    }
    Some(j)
}

/// `scanstring_unicode` (strict): `end` is just after the opening quote.
fn string(p: &[char], mut end: usize) -> Result<usize, Stop> {
    let begin = end - 1;
    let hex = |j: usize| p.get(j).is_some_and(char::is_ascii_hexdigit);
    let value_of = |from: usize| {
        p[from..from + 4]
            .iter()
            .fold(0u32, |n, c| n * 16 + c.to_digit(16).unwrap_or(0))
    };
    loop {
        let mut next = end;
        let mut c = '\0';
        while next < p.len() {
            c = p[next];
            if c == '"' || c == '\\' {
                break;
            }
            if u32::from(c) <= 0x1f {
                return Err(("Invalid control character at", next));
            }
            next += 1;
        }
        if next >= p.len() {
            return Err(("Unterminated string starting at", begin));
        }
        next += 1;
        if c == '"' {
            return Ok(next);
        }
        if next == p.len() {
            return Err(("Unterminated string starting at", begin));
        }
        if p[next] != 'u' {
            end = next + 1;
            if !matches!(p[next], '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(("Invalid \\escape", end - 2));
            }
            continue;
        }
        next += 1;
        end = next + 4;
        if end >= p.len() {
            return Err(("Invalid \\uXXXX escape", next - 1));
        }
        if !(next..end).all(hex) {
            return Err(("Invalid \\uXXXX escape", end - 5));
        }
        let first = value_of(next);
        next = end;
        if (0xd800..=0xdbff).contains(&first)
            && end + 6 < p.len()
            && p[next] == '\\'
            && p[next + 1] == 'u'
        {
            end += 6;
            if !(next + 2..end).all(hex) {
                return Err(("Invalid \\uXXXX escape", end - 5));
            }
            if !(0xdc00..=0xdfff).contains(&value_of(next + 2)) {
                end -= 6;
            }
        }
    }
}

/// `_parse_object_unicode`: `i` is just after `{`.
fn object(p: &[char], i: usize) -> Result<usize, Stop> {
    const NAME: &str = "Expecting property name enclosed in double quotes";
    let mut i = skip_ws(p, i);
    if p.get(i) == Some(&'}') {
        return Ok(i + 1);
    }
    loop {
        if p.get(i) != Some(&'"') {
            return Err((NAME, i));
        }
        i = skip_ws(p, string(p, i + 1)?);
        if p.get(i) != Some(&':') {
            return Err(("Expecting ':' delimiter", i));
        }
        i = skip_ws(p, i + 1);
        i = skip_ws(p, value(p, i)?);
        match p.get(i) {
            Some('}') => return Ok(i + 1),
            Some(',') => i = skip_ws(p, i + 1),
            _ => return Err(("Expecting ',' delimiter", i)),
        }
    }
}

/// `_parse_array_unicode`: `i` is just after `[`.
fn array(p: &[char], i: usize) -> Result<usize, Stop> {
    let mut i = skip_ws(p, i);
    if p.get(i) == Some(&']') {
        return Ok(i + 1);
    }
    loop {
        i = skip_ws(p, value(p, i)?);
        match p.get(i) {
            Some(']') => return Ok(i + 1),
            Some(',') => i = skip_ws(p, i + 1),
            _ => return Err(("Expecting ',' delimiter", i)),
        }
    }
}

/// `open(path, encoding="utf-8").read()`: the text with `\r\n` and `\r`
/// read as `\n` (universal newlines), or Python's error for it: its class
/// and `str(e)`.
pub fn read_text_file(path: &str) -> Result<String, Failed> {
    let bytes = std::fs::read(path).map_err(|e| os_error(&e, path))?;
    let text = decode_utf8(&bytes)?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `bytes.decode("utf-8")`: the text, or the `UnicodeDecodeError`.
pub fn decode_utf8(bytes: &[u8]) -> Result<String, Failed> {
    match std::str::from_utf8(bytes) {
        Ok(t) => Ok(t.to_string()),
        Err(e) => {
            let at = e.valid_up_to();
            let first = bytes[at];
            let (end, why) = match e.error_len() {
                None => (bytes.len(), "unexpected end of data"),
                Some(_) if (0xc2..=0xf4).contains(&first) => (at + 1, "invalid continuation byte"),
                Some(_) => (at + 1, "invalid start byte"),
            };
            let place = if end - at == 1 {
                format!("byte 0x{first:02x} in position {at}")
            } else {
                format!("bytes in position {at}-{}", end - 1)
            };
            Err(Failed {
                class: "UnicodeDecodeError",
                message: format!("'utf-8' codec can't decode {place}: {why}"),
            })
        }
    }
}

/// An `OSError` as Python names it: its class and `str(e)`, with the path.
pub fn os_error(e: &std::io::Error, path: &str) -> Failed {
    let code = e.raw_os_error().unwrap_or(0);
    let class = match code {
        2 => "FileNotFoundError",
        13 | 1 => "PermissionError",
        21 => "IsADirectoryError",
        20 => "NotADirectoryError",
        17 => "FileExistsError",
        _ => "OSError",
    };
    Failed {
        class,
        message: format!(
            "[Errno {code}] {}: {}",
            strerror(code),
            crate::out::py::repr(&Value::Str(path.into()))
        ),
    }
}

/// The C library's words for an errno, as Python prints them (the
/// standard library's text without its ` (os error N)` tail). The codes
/// above are the same on Linux and macOS.
fn strerror(code: i32) -> String {
    let text = std::io::Error::from_raw_os_error(code).to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> String {
        loads(text).unwrap_err().message
    }

    /// Each pinned against Python 3.12's `json.loads`.
    #[test]
    fn python_s_words_and_places() {
        for (text, want) in [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("  ", "Expecting value: line 1 column 3 (char 2)"),
            ("x", "Expecting value: line 1 column 1 (char 0)"),
            (
                "{",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            (
                "{\"a\"",
                "Expecting ':' delimiter: line 1 column 5 (char 4)",
            ),
            ("{\"a\":", "Expecting value: line 1 column 6 (char 5)"),
            (
                "{\"a\":1",
                "Expecting ',' delimiter: line 1 column 7 (char 6)",
            ),
            (
                "{\"a\":1,}",
                "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)",
            ),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            ("[1]x", "Extra data: line 1 column 4 (char 3)"),
            (
                "\"abc",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            (
                "\"a\tb\"",
                "Invalid control character at: line 1 column 3 (char 2)",
            ),
            ("\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
            (
                "\"\\uzzzz\"",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            (
                "\"\\u12\"",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            (
                "{\n  \"ops\": [1,\n  ]\n}",
                "Expecting value: line 3 column 3 (char 17)",
            ),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            (
                "\u{feff}[]",
                "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
            ),
            (
                "[\"é\" x]",
                "Expecting ',' delimiter: line 1 column 6 (char 5)",
            ),
        ] {
            assert_eq!(err(text), want, "{text:?}");
        }
    }

    #[test]
    fn what_python_reads_is_read() {
        for text in [
            "[]",
            " {\"ops\": [1, 2.5, NaN, -Infinity]} ",
            "\"\\ud83d\\ude00\"",
            "1e5",
            "-0",
        ] {
            assert!(loads(text).is_ok(), "{text:?}");
        }
    }

    #[test]
    fn a_file_that_is_not_utf8() {
        let e = |b: &[u8]| decode_utf8(b).unwrap_err().message;
        assert_eq!(
            e(b"ab\xff"),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
        assert_eq!(
            e(b"a\xc3x"),
            "'utf-8' codec can't decode byte 0xc3 in position 1: invalid continuation byte"
        );
        assert_eq!(
            e(b"a\xe2\x82"),
            "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data"
        );
    }
}
