//! The text of the `json.JSONDecodeError` that Python 3.12's `json.loads`
//! raises on a text that is not JSON, as `str(e)` prints it:
//! `Expecting value: line 1 column 1 (char 0)`. The shell's readers print
//! it in their own words (`Failed to parse daemon response: {e}`), so the
//! message and its place must be Python's exactly: the scan below follows
//! CPython's C scanner (`Modules/_json.c`) and `json/decoder.py`.
//!
//! Python 3.13 and later word a trailing comma their own way (`Illegal
//! trailing comma before end of object`, at the comma); this prints 3.12's
//! words, as the families print 3.12's tracebacks.
//!
//! Places count code points, as Python's do. Only for a text
//! [`crate::out::json::parse`] refused: the two read the same language.
//!
//! Written here for the tunnel family; it belongs with the shared JSON
//! reader once another family needs it.

/// `str(e)` for the `JSONDecodeError` `json.loads(text)` raises, or `None`
/// when this scan finds no error (the text is JSON as Python reads it,
/// or it fails some other way, too deep for one).
pub fn decode_error(text: &str) -> Option<String> {
    let s: Vec<char> = text.chars().collect();
    let (msg, pos) = scan_document(&s)?;
    Some(format(&s, msg, pos))
}

/// `'%s: line %d column %d (char %d)'`.
fn format(s: &[char], msg: &str, pos: usize) -> String {
    let before = &s[..pos.min(s.len())];
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let col = match before.iter().rposition(|c| *c == '\n') {
        Some(i) => pos - i,
        None => pos + 1,
    };
    format!("{msg}: line {line} column {col} (char {pos})")
}

type Fail = (&'static str, usize);

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn skip_ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && is_ws(s[i]) {
        i += 1;
    }
    i
}

/// `json.loads` on a `str`, then `JSONDecoder.decode`.
fn scan_document(s: &[char]) -> Option<Fail> {
    if s.first() == Some(&'\u{feff}') {
        return Some(("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = skip_ws(s, 0);
    let end = match value(s, start, 0) {
        Ok(end) => end,
        Err(Stop::Value(at)) => return Some(("Expecting value", at)),
        Err(Stop::Error(f)) => return Some(f),
        Err(Stop::Deep) => return None,
    };
    let end = skip_ws(s, end);
    (end != s.len()).then_some(("Extra data", end))
}

enum Stop {
    /// `StopIteration(at)`: no value starts here.
    Value(usize),
    Error(Fail),
    /// Nested past what this scan follows (Python raises RecursionError).
    Deep,
}

const MAX_DEPTH: usize = 900;

fn starts(s: &[char], i: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    s.len() >= i + w.len() && s[i..i + w.len()] == w[..]
}

/// `scan_once_unicode`: the end of the value at `i`.
fn value(s: &[char], i: usize, depth: usize) -> Result<usize, Stop> {
    if depth > MAX_DEPTH {
        return Err(Stop::Deep);
    }
    let Some(&c) = s.get(i) else {
        return Err(Stop::Value(i));
    };
    match c {
        '"' => string(s, i + 1),
        '{' => object(s, i + 1, depth + 1),
        '[' => array(s, i + 1, depth + 1),
        'n' if starts(s, i, "null") => Ok(i + 4),
        't' if starts(s, i, "true") => Ok(i + 4),
        'f' if starts(s, i, "false") => Ok(i + 5),
        'N' if starts(s, i, "NaN") => Ok(i + 3),
        'I' if starts(s, i, "Infinity") => Ok(i + 8),
        '-' if starts(s, i, "-Infinity") => Ok(i + 9),
        _ => number(s, i),
    }
}

/// `_match_number_unicode`.
fn number(s: &[char], start: usize) -> Result<usize, Stop> {
    let mut i = start;
    if s.get(i) == Some(&'-') {
        i += 1;
    }
    match s.get(i) {
        Some('1'..='9') => {
            i += 1;
            while matches!(s.get(i), Some('0'..='9')) {
                i += 1;
            }
        }
        Some('0') => i += 1,
        _ => return Err(Stop::Value(start)),
    }
    if s.get(i) == Some(&'.') && matches!(s.get(i + 1), Some('0'..='9')) {
        i += 2;
        while matches!(s.get(i), Some('0'..='9')) {
            i += 1;
        }
    }
    if matches!(s.get(i), Some('e' | 'E')) {
        let e = i;
        i += 1;
        if matches!(s.get(i), Some('+' | '-')) {
            i += 1;
        }
        if matches!(s.get(i), Some('0'..='9')) {
            while matches!(s.get(i), Some('0'..='9')) {
                i += 1;
            }
        } else {
            i = e;
        }
    }
    Ok(i)
}

fn hex(c: char) -> Option<u32> {
    c.to_digit(16)
}

/// `scanstring_unicode` from `begin`, just past the opening quote.
fn string(s: &[char], begin: usize) -> Result<usize, Stop> {
    let len = s.len();
    let unterminated = || Stop::Error(("Unterminated string starting at", begin - 1));
    let mut end = begin;
    loop {
        let mut next = end;
        let mut c = '\0';
        while next < len {
            c = s[next];
            if c == '"' || c == '\\' {
                break;
            }
            if (c as u32) <= 0x1f {
                return Err(Stop::Error(("Invalid control character at", next)));
            }
            next += 1;
        }
        if next == len {
            return Err(unterminated());
        }
        if c == '"' {
            return Ok(next + 1);
        }
        next += 1;
        if next == len {
            return Err(unterminated());
        }
        let e = s[next];
        if e != 'u' {
            end = next + 1;
            if !matches!(e, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(Stop::Error(("Invalid \\escape", end - 2)));
            }
            continue;
        }
        next += 1;
        end = next + 4;
        if end >= len {
            return Err(Stop::Error(("Invalid \\uXXXX escape", next - 1)));
        }
        let mut u = 0u32;
        while next < end {
            let Some(d) = hex(s[next]) else {
                return Err(Stop::Error(("Invalid \\uXXXX escape", end - 5)));
            };
            u = (u << 4) | d;
            next += 1;
        }
        if (0xd800..=0xdbff).contains(&u) && end + 6 < len && s[next] == '\\' && s[next + 1] == 'u'
        {
            next += 2;
            end += 6;
            let mut u2 = 0u32;
            while next < end {
                let Some(d) = hex(s[next]) else {
                    return Err(Stop::Error(("Invalid \\uXXXX escape", end - 5)));
                };
                u2 = (u2 << 4) | d;
                next += 1;
            }
            if !(0xdc00..=0xdfff).contains(&u2) {
                end -= 6;
            }
        }
    }
}

/// `_parse_object_unicode` from just past `{`.
fn object(s: &[char], i: usize, depth: usize) -> Result<usize, Stop> {
    let mut i = skip_ws(s, i);
    if s.get(i) == Some(&'}') {
        return Ok(i + 1);
    }
    loop {
        if s.get(i) != Some(&'"') {
            return Err(Stop::Error((
                "Expecting property name enclosed in double quotes",
                i,
            )));
        }
        i = skip_ws(s, string(s, i + 1)?);
        if s.get(i) != Some(&':') {
            return Err(Stop::Error(("Expecting ':' delimiter", i)));
        }
        i = skip_ws(s, i + 1);
        i = skip_ws(s, value(s, i, depth)?);
        match s.get(i) {
            Some('}') => return Ok(i + 1),
            Some(',') => i = skip_ws(s, i + 1),
            _ => return Err(Stop::Error(("Expecting ',' delimiter", i))),
        }
    }
}

/// `_parse_array_unicode` from just past `[`.
fn array(s: &[char], i: usize, depth: usize) -> Result<usize, Stop> {
    let mut i = skip_ws(s, i);
    if s.get(i) == Some(&']') {
        return Ok(i + 1);
    }
    loop {
        i = skip_ws(s, value(s, i, depth)?);
        match s.get(i) {
            Some(']') => return Ok(i + 1),
            Some(',') => i = skip_ws(s, i + 1),
            _ => return Err(Stop::Error(("Expecting ',' delimiter", i))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_error;

    /// Python 3.12's `str(e)` for each, from `json.loads`.
    #[test]
    fn python_s_words_and_places() {
        for (text, want) in [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("  \n", "Expecting value: line 2 column 1 (char 3)"),
            ("<html>", "Expecting value: line 1 column 1 (char 0)"),
            (
                "{",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            ("[", "Expecting value: line 1 column 2 (char 1)"),
            ("[1,", "Expecting value: line 1 column 4 (char 3)"),
            ("[1", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            (
                "{\"a\":1,}",
                "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)",
            ),
            (
                "{\"a\" 1}",
                "Expecting ':' delimiter: line 1 column 6 (char 5)",
            ),
            (
                "{\"a\":1 \"b\"}",
                "Expecting ',' delimiter: line 1 column 8 (char 7)",
            ),
            ("{\"a\":}", "Expecting value: line 1 column 6 (char 5)"),
            (
                "{\"a",
                "Unterminated string starting at: line 1 column 2 (char 1)",
            ),
            (
                "\"a\u{1}\"",
                "Invalid control character at: line 1 column 3 (char 2)",
            ),
            ("\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
            (
                "\"\\u12x4\"",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            ("{} x", "Extra data: line 1 column 4 (char 3)"),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("nul", "Expecting value: line 1 column 1 (char 0)"),
            ("{\"é\":\n x}", "Expecting value: line 2 column 2 (char 7)"),
            (
                "\u{feff}{}",
                "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
            ),
        ] {
            assert_eq!(decode_error(text).as_deref(), Some(want), "{text:?}");
        }
        for ok in [
            "{}",
            "[1, 2.5e3, -0, NaN, -Infinity, \"\\ud83d\\ude00\"]",
            " null ",
        ] {
            assert_eq!(decode_error(ok), None, "{ok:?}");
        }
    }
}
