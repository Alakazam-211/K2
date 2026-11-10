//! The text rules: org names, the purpose paragraph, the 5-line bulletin,
//! members' updates and heads' asks (prd-org-chart-v1 §4, §10.1).
//!
//! Every bulletin line reaches every agent on the server, so it is plain
//! text: control characters (ANSI escapes included) are stripped, and a line
//! may not start with a K2 delivery stamp (`[thread:`, `[from `, `[org:` …),
//! so it can't pose as a message from K2 or a person.

use super::OrgError;

/// At most this many lines in an org bulletin.
pub const BULLETIN_MAX_LINES: usize = 5;
/// At most this many characters on one bulletin line.
pub const BULLETIN_MAX_CHARS: usize = 160;
/// Org names: 1–40 characters.
pub const NAME_MAX_CHARS: usize = 40;
/// The purpose: a short paragraph (Rosson 2026-11-10: "cap it, e.g. 400").
pub const PURPOSE_MAX_CHARS: usize = 400;
/// A member's update to its head.
pub const UPDATE_MAX_CHARS: usize = 500;
/// A head's question to its members.
pub const ASK_MAX_CHARS: usize = 500;
/// The one line of why on a reach ask.
pub const WHY_MAX_CHARS: usize = 300;

/// Stamps K2 and people's messages start with. A bulletin line, a purpose
/// or an org name may not start with one.
const STAMPS: &[&str] = &["[thread:", "[from ", "[from:", "[org:", "[org ", "[project:", "[feedback:", "[guest", "[k2", "[mail", "[permission"];

/// Drop ANSI escape sequences and every control character but `\n`.
pub fn strip_controls(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ params… final byte (@–~). Other escapes: ESC + one.
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        if c == '\n' || !c.is_control() {
            out.push(c);
        } else if c == '\t' {
            out.push(' ');
        }
    }
    out
}

/// One line: controls stripped, newlines and runs of spaces folded to one
/// space, trimmed.
pub fn single_line(raw: &str) -> String {
    strip_controls(raw).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn starts_with_stamp(line: &str) -> bool {
    let l = line.trim_start().to_ascii_lowercase();
    STAMPS.iter().any(|s| l.starts_with(s))
}

fn stamp_error(what: &str) -> OrgError {
    OrgError::new(
        400,
        "bulletin_stamp",
        format!("{what} can't start with a K2 stamp like [thread:…], [from …] or [org:…]; write plain text."),
    )
}

/// An org name: 1–40 characters on one line, no stamp.
pub fn org_name(raw: &str) -> Result<String, OrgError> {
    let name = single_line(raw);
    let n = name.chars().count();
    if n == 0 || n > NAME_MAX_CHARS {
        return Err(OrgError::new(
            400,
            "org_name_invalid",
            format!("An org name is 1 to {NAME_MAX_CHARS} characters; this one has {n}."),
        ));
    }
    if starts_with_stamp(&name) {
        return Err(stamp_error("An org name"));
    }
    Ok(name)
}

/// The purpose paragraph: one paragraph of at most 400 characters (empty
/// clears it).
pub fn purpose(raw: &str) -> Result<String, OrgError> {
    let p = single_line(raw);
    let n = p.chars().count();
    if n > PURPOSE_MAX_CHARS {
        return Err(OrgError::new(
            400,
            "purpose_too_long",
            format!("An org's purpose is at most {PURPOSE_MAX_CHARS} characters; this one has {n}. Cut {} and send it again.", n - PURPOSE_MAX_CHARS),
        ));
    }
    if starts_with_stamp(&p) {
        return Err(stamp_error("A purpose"));
    }
    Ok(p)
}

/// The bulletin: each argument may hold several lines (a file's body);
/// blank lines are dropped. At most 5 lines of at most 160 characters each,
/// and not empty.
pub fn bulletin(parts: &[String]) -> Result<Vec<String>, OrgError> {
    let mut lines: Vec<String> = Vec::new();
    for part in parts {
        for raw in strip_controls(part).split('\n') {
            let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if !line.is_empty() {
                lines.push(line);
            }
        }
    }
    if lines.is_empty() {
        return Err(OrgError::new(
            400,
            "bulletin_empty",
            "An org bulletin needs at least one line: k2 organization bulletin set \"<line 1>\" \"<line 2>\" …",
        ));
    }
    if lines.len() > BULLETIN_MAX_LINES {
        let n = lines.len();
        return Err(OrgError::new(
            400,
            "bulletin_too_long",
            format!(
                "An org bulletin is at most {BULLETIN_MAX_LINES} lines; this one has {n}. Cut {} line{} and send it again.",
                n - BULLETIN_MAX_LINES,
                if n - BULLETIN_MAX_LINES == 1 { "" } else { "s" }
            ),
        ));
    }
    for (i, line) in lines.iter().enumerate() {
        let n = line.chars().count();
        if n > BULLETIN_MAX_CHARS {
            return Err(OrgError::new(
                400,
                "bulletin_line_too_long",
                format!("Line {} is {n} characters; the most is {BULLETIN_MAX_CHARS}.", i + 1),
            ));
        }
        if starts_with_stamp(line) {
            return Err(stamp_error(&format!("Line {}", i + 1)));
        }
    }
    Ok(lines)
}

/// A capped one-line text (an update, an ask, a why). `what` names it in
/// the error; `code` is the error's code.
pub fn capped(raw: &str, max: usize, what: &str, code: &'static str) -> Result<String, OrgError> {
    let t = single_line(raw);
    let n = t.chars().count();
    if n == 0 {
        return Err(OrgError::new(400, code, format!("{what} is empty.")));
    }
    if n > max {
        return Err(OrgError::new(400, code, format!("{what} is at most {max} characters; this one has {n}.")));
    }
    Ok(t)
}

/// The org name's uniqueness key.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}
