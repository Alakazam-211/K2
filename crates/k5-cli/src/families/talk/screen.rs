//! What `talk` reads off a screen, as the shell's helpers read it: its
//! small Pythons (`_talk_read_raw`, `_talk_parse_tabbar`,
//! `_talk_trim_screen`, `_talk_classify`, the session pick in
//! `resolve_write_session_id`) and its grep and read lines
//! (`_talk_parse_total`, the scout summary's option lines, the
//! `--answers` list).
//!
//! The grep and `[[:space:]]` forms follow the locale as GNU grep and bash
//! do: with a UTF-8 `LC_ALL`, `LC_CTYPE` or `LANG` a character is a
//! character; without one (the C locale) a byte is, so the scout
//! summary's `❯?` only makes the last of its three bytes optional, and a
//! line needs the first two to be listed. That is the shell's behaviour,
//! copied (a bash bug, reported). Python's `\d` here is ASCII's digits
//! only; Python takes any Unicode decimal digit.

use crate::families::py_space;
use crate::out::json::{self, Value};
use crate::out::py;

/// The C locale's `[[:space:]]`.
fn c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// glibc's `iswspace` in a UTF-8 locale.
fn utf8_space(c: char) -> bool {
    matches!(c, ' ' | '\t'..='\r' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{205f}' | '\u{3000}')
}

/// True when the locale's character type is UTF-8 (`LC_ALL`, else
/// `LC_CTYPE`, else `LANG`, the first that is set and not empty).
pub fn utf8_locale() -> bool {
    let v = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    v.contains("utf-8") || v.contains("utf8")
}

/// Python's `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let end = match c {
            '\r' => {
                if let Some((_, '\n')) = it.peek() {
                    it.next();
                    Some(i + 2)
                } else {
                    Some(i + 1)
                }
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => Some(i + c.len_utf8()),
            _ => None,
        };
        if let Some(end) = end {
            out.push(&text[start..i]);
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `$(…)`: the text less its trailing newlines.
pub fn held(mut text: String) -> String {
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// A JSON value as Python's `str()`, or `None` where printing it would
/// raise (half of a surrogate pair).
fn printable(v: &Value) -> Option<String> {
    match v {
        Value::Str(t) => t.as_str().map(str::to_string),
        other => Some(py::str_of(other)),
    }
}

/// What `for x in v` walks: a list's items, a string's characters, an
/// object's keys. `None` where it raises.
fn iterate(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Array(items) => Some(items.clone()),
        Value::Str(t) => Some(
            t.as_str()?
                .chars()
                .map(|c| Value::Str(json::Text::from(c.to_string())))
                .collect(),
        ),
        Value::Object(pairs) => Some(pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect()),
        _ => None,
    }
}

fn get<'a>(v: &'a Value, key: &str) -> Option<Option<&'a Value>> {
    match v {
        Value::Object(pairs) => Some(crate::families::get(pairs, key)),
        _ => None,
    }
}

/// `_talk_read_raw`'s Python on an answer, held by `$(…)`: `lines`
/// joined by newlines; empty on anything that raises.
pub fn read_raw(body: &[u8]) -> String {
    let Ok(d) = json::parse(&String::from_utf8_lossy(body)) else {
        return String::new();
    };
    let Some(lines) = get(&d, "lines") else {
        return String::new();
    };
    let items = match lines {
        None => Vec::new(),
        Some(v) => match iterate(v) {
            Some(items) => items,
            None => return String::new(),
        },
    };
    let mut parts = Vec::with_capacity(items.len());
    for item in &items {
        // `'\n'.join` takes strings only.
        let Value::Str(t) = item else {
            return String::new();
        };
        let Some(s) = t.as_str() else {
            return String::new();
        };
        parts.push(s.to_string());
    }
    held(parts.join("\n") + "\n")
}

/// `cmd_terminal_read`'s Python: each of `lines`, one per line. `None`
/// where it raises (and prints nothing), so the shell asks again and
/// prints the raw answer.
pub fn terminal_lines(body: &[u8]) -> Option<String> {
    let d = json::parse(&String::from_utf8_lossy(body)).ok()?;
    let items = match get(&d, "lines")? {
        None => Vec::new(),
        Some(v) => iterate(v)?,
    };
    let mut out = String::new();
    for item in &items {
        out.push_str(&printable(item)?);
        out.push('\n');
    }
    Some(out)
}

/// A capped read's note in `cmd_terminal_read`'s answer (the lines were
/// read first): `None` when there is none (not a string), `Some(None)`
/// where its `os.write(3, (...).encode())` raises (a lone surrogate),
/// else the line it writes on stderr (without its newline).
pub fn terminal_note(body: &[u8]) -> Option<Option<String>> {
    let d = json::parse(&String::from_utf8_lossy(body)).ok()?;
    match get(&d, "note")? {
        Some(Value::Str(t)) => Some(
            t.as_str()
                .map(|s| format!("{}{s}", super::super::terminal::READ_NOTE)),
        ),
        _ => None,
    }
}

/// The tab bar's line test.
fn is_tab_bar(line: &str) -> bool {
    (line.contains('☐') || line.contains('☒'))
        && (line.contains('✔') || line.contains('✓') || line.contains("Submit"))
}

/// `_talk_parse_tabbar`: the questions' names, in order, without Submit.
pub fn tabbar(screen: &str) -> Vec<String> {
    let Some(tab) = splitlines(screen).into_iter().find(|l| is_tab_bar(l)) else {
        return Vec::new();
    };
    const MARKS: [char; 4] = ['☐', '☒', '✔', '✓'];
    let mut names = Vec::new();
    let chars: Vec<char> = tab.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let marker = chars[i];
        if !MARKS.contains(&marker) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && !MARKS.contains(&chars[j]) && !matches!(chars[j], '→' | '←') {
            j += 1;
        }
        let raw: String = chars[i + 1..j].iter().collect();
        let name = raw
            .trim_matches(py_space)
            .trim_matches(|c: char| c == '→' || c == '←')
            .trim_matches(py_space)
            .to_string();
        if matches!(marker, '☐' | '☒') && !name.is_empty() && name != "Submit" {
            names.push(name);
        }
        i = j;
    }
    names
}

/// The trim's option line: `^\s*[❯>]?\s*\d+[.)]\s`.
fn is_option(line: &str) -> bool {
    let s = line.trim_start_matches(py_space);
    let s = s
        .strip_prefix('❯')
        .or_else(|| s.strip_prefix('>'))
        .unwrap_or(s);
    let s = s.trim_start_matches(py_space);
    let rest = s.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == s.len() {
        return false;
    }
    let Some(rest) = rest.strip_prefix(['.', ')']) else {
        return false;
    };
    rest.chars().next().is_some_and(py_space)
}

/// `_talk_trim_screen`, held by `$(…)`: from the tab bar through the last
/// option line; the whole screen when it can't be bounded.
pub fn trim(screen: &str) -> String {
    let lines = splitlines(screen);
    let start = lines.iter().position(|l| is_tab_bar(l));
    let end = lines.iter().rposition(|l| is_option(l));
    let text = match (start, end) {
        (Some(s), Some(e)) if e >= s => lines[s..=e].join("\n"),
        _ => lines.join("\n"),
    };
    held(text)
}

/// One `[[:space:]]+` (at least one) at `s`'s start, by the locale.
fn spaces(s: &[u8], utf8: bool) -> usize {
    if utf8 {
        let Ok(text) = std::str::from_utf8(s) else {
            return s.iter().take_while(|b| c_space(**b)).count();
        };
        text.chars()
            .take_while(|c| utf8_space(*c))
            .map(char::len_utf8)
            .sum()
    } else {
        s.iter().take_while(|b| c_space(**b)).count()
    }
}

fn digits(s: &[u8]) -> usize {
    s.iter().take_while(|b| b.is_ascii_digit()).count()
}

/// `_talk_parse_total`: Y of the first `Question X of Y` (any case), or
/// empty.
pub fn total(screen: &str, utf8: bool) -> String {
    for line in screen.split('\n') {
        let b = line.as_bytes();
        for at in 0..b.len() {
            if b.len() - at < 8 || !b[at..at + 8].eq_ignore_ascii_case(b"question") {
                continue;
            }
            let mut i = at + 8;
            let n = spaces(&b[i..], utf8);
            if n == 0 {
                continue;
            }
            i += n;
            let n = digits(&b[i..]);
            if n == 0 {
                continue;
            }
            i += n;
            let n = spaces(&b[i..], utf8);
            if n == 0 {
                continue;
            }
            i += n;
            if b.len() < i + 2 || !b[i..i + 2].eq_ignore_ascii_case(b"of") {
                continue;
            }
            i += 2;
            let n = spaces(&b[i..], utf8);
            if n == 0 {
                continue;
            }
            i += n;
            let n = digits(&b[i..]);
            if n == 0 {
                continue;
            }
            return String::from_utf8_lossy(&b[i..i + n]).into_owned();
        }
    }
    String::new()
}

/// The scout summary's option lines: `grep -E
/// '^[[:space:]]*❯?[[:space:]]*[0-9]+[\.\)][[:space:]]'` over the screen,
/// each through `sed 's/^[[:space:]]*/   /'`.
pub fn option_lines(screen: &str, utf8: bool) -> String {
    let mut out = String::new();
    for line in screen.split('\n') {
        let b = line.as_bytes();
        let lead = spaces(b, utf8);
        let mut i = lead;
        if utf8 {
            if b[i..].starts_with("❯".as_bytes()) {
                i += "❯".len();
            }
        } else {
            // `❯?` in bytes: E2 9D, then AF or not.
            if !b[i..].starts_with(&[0xe2, 0x9d]) {
                continue;
            }
            i += 2;
            if b.get(i) == Some(&0xaf) {
                i += 1;
            }
        }
        i += spaces(&b[i..], utf8);
        let n = digits(&b[i..]);
        if n == 0 {
            continue;
        }
        i += n;
        if !matches!(b.get(i), Some(b'.' | b')')) {
            continue;
        }
        i += 1;
        if spaces(&b[i..], utf8) == 0 {
            continue;
        }
        out.push_str("   ");
        out.push_str(&String::from_utf8_lossy(&b[lead..]));
        out.push('\n');
    }
    out
}

/// `IFS=',' read -ra ans <<< "$csv"`, then each answer less every
/// `[[:space:]]`, the empty ones dropped.
pub fn answers(csv: &str, utf8: bool) -> Vec<String> {
    let line = csv.split('\n').next().unwrap_or("");
    let mut fields: Vec<&str> = line.split(',').collect();
    // bash drops one empty field at the end.
    if fields.last() == Some(&"") {
        fields.pop();
    }
    fields
        .into_iter()
        .map(|f| {
            if utf8 {
                f.chars().filter(|c| !utf8_space(*c)).collect::<String>()
            } else {
                f.chars()
                    .filter(|c| !(c.is_ascii() && c_space(*c as u8)))
                    .collect()
            }
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// `_talk_classify`'s Python, held by `$(…)`: `<is_hitl>|<source>`, or
/// empty where it raises.
pub fn classify(body: &[u8]) -> String {
    let Ok(d) = json::parse(&String::from_utf8_lossy(body)) else {
        return String::new();
    };
    let Some(h) = get(&d, "is_hitl") else {
        return String::new();
    };
    let hs = match h {
        None | Some(Value::Null) => "null",
        Some(v) if py::truthy(v) => "true",
        Some(_) => "false",
    };
    let source = match get(&d, "source").flatten() {
        None => String::new(),
        // `hs + '|' + s` takes a string only.
        Some(Value::Str(t)) => match t.as_str() {
            Some(s) => s.to_string(),
            None => return String::new(),
        },
        Some(_) => return String::new(),
    };
    held(format!("{hs}|{source}\n"))
}

/// The resolve's Python: `print(json.load(stdin).get("path",""))`, held.
pub fn resolved_path(body: &[u8]) -> String {
    let Ok(d) = json::parse(&String::from_utf8_lossy(body)) else {
        return String::new();
    };
    match get(&d, "path") {
        None | Some(None) => String::new(),
        Some(Some(v)) => printable(v).map_or_else(String::new, |s| held(s + "\n")),
    }
}

/// The session pick's Python, held: the first live v2 session (or row),
/// the agent's when one matches.
pub fn session_id(body: &[u8], agent: &str) -> String {
    let rows = match json::parse(&String::from_utf8_lossy(body)) {
        Ok(Value::Array(rows)) => rows,
        _ => Vec::new(),
    };
    // `r.get(...)` on a row that is not an object raises.
    let mut objects = Vec::with_capacity(rows.len());
    for r in &rows {
        let Value::Object(pairs) = r else {
            return String::new();
        };
        objects.push(pairs);
    }
    let v2: Vec<_> = objects
        .iter()
        .filter(|p| crate::families::get(p, "isV2").is_some_and(py::truthy))
        .collect();
    let mut pool: Vec<_> = if v2.is_empty() {
        objects.iter().collect()
    } else {
        v2
    };
    if !agent.is_empty() {
        let matched: Vec<_> = pool
            .iter()
            .copied()
            .filter(|p| {
                matches!(crate::families::get(p, "agentName"),
                    Some(Value::Str(t)) if t.as_str() == Some(agent))
            })
            .collect();
        if !matched.is_empty() {
            pool = matched;
        }
    }
    let Some(first) = pool.first() else {
        return String::new();
    };
    match crate::families::get(first, "sessionId") {
        None => String::new(),
        Some(v) => printable(v).map_or_else(String::new, |s| held(s + "\n")),
    }
}

/// `printf '%s' "$target" | grep -qiE
/// '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'`:
/// any line of the target that is a UUID.
pub fn is_uuid(s: &str) -> bool {
    s.split('\n').any(|line| {
        let parts: Vec<&str> = line.split('-').collect();
        parts.len() == 5
            && parts
                .iter()
                .zip([8, 4, 4, 4, 12])
                .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}
