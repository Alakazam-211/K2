//! The permission note: the closing line a refusal's `permission` object
//! gives, as the shell CLI finds and prints it (`cli/src/cell-socket.sh`,
//! `BEGIN_CLI_PERMISSION_NOTE`).
//!
//! The shell hands each answer's text to `_permission_note`, which keeps
//! the lines `_permission_closing_lines` (Python) prints for it in a note
//! file; when the command exits non-zero the router's EXIT trap prints
//! the file on stderr, so a refusal's last stderr line is it. The rule:
//!
//! 1. The text must hold `"permission"` as written (a plain substring
//!    test, before any parsing).
//! 2. A trailing `\n` and three digits is cut off (`cli_send_json`'s
//!    status line); Python's `$` also takes it before one final `\n`.
//! 3. It must parse as JSON (Python's `json.loads`), be an object, and its
//!    `permission` an object.
//! 4. A non-empty string `command`: `Ask with: <command>`.
//! 5. Else `requestable` exactly `false`: its non-empty string `hint`, if
//!    any, then `It can't be requested.`
//!
//! What bash then does to Python's output: `$(…)` drops every trailing
//! newline (and any NUL), and the note file adds one newline back.
//!
//! Python fails on a string it can't write as UTF-8 (a lone surrogate
//! from a `\ud83d` escape) and the shell then notes nothing; so does this.
//! A text that is not UTF-8 gives no note here; Python would read it with
//! surrogate escapes. The daemon writes UTF-8 JSON.
//!
//! Not here: the Org Chart bulletin note and the dead-passport check the
//! same shell function keeps; and when to print (a failed command only,
//! the last note of the run, `_K2_PERMISSION_QUIET`), which is the
//! caller's.

use crate::out::json::{self, Value};
use std::io::{self, Write};

/// The note: one or two lines, the last being the closing line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    lines: Vec<String>,
}

impl Note {
    /// The lines, as Python printed them.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// What the shell prints on stderr for it: the lines joined by
    /// newlines, trailing newlines and NULs dropped (`$(…)`), and one
    /// newline at the end.
    pub fn text(&self) -> String {
        let joined: String = self
            .lines
            .join("\n")
            .chars()
            .filter(|c| *c != '\0')
            .collect();
        format!("{}\n", joined.trim_end_matches('\n'))
    }

    /// Prints [`Note::text`] on `w`.
    pub fn print(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(self.text().as_bytes())?;
        w.flush()
    }
}

/// The note for a text the shell would hand `_permission_note`, or none.
pub fn note(text: &[u8]) -> Option<Note> {
    if !contains(text, b"\"permission\"") {
        return None;
    }
    let text = std::str::from_utf8(text).ok()?;
    let value = json::parse(strip_status(text)).ok()?;
    let permission = get(&value, "permission")?;
    if !matches!(permission, Value::Object(_)) {
        return None;
    }
    let mut lines = Vec::new();
    match get(permission, "command") {
        Some(Value::Str(cmd)) if *cmd != json::Text::from("") => {
            lines.push(format!("Ask with: {}", cmd.as_str()?));
            return Some(Note { lines });
        }
        _ => {}
    }
    if get(permission, "requestable") != Some(&Value::Bool(false)) {
        return None;
    }
    if let Some(Value::Str(hint)) = get(permission, "hint") {
        if *hint != json::Text::from("") {
            lines.push(hint.as_str()?.to_string());
        }
    }
    lines.push("It can't be requested.".to_string());
    Some(Note { lines })
}

/// Python's `re.match(r"(?s)(.*)\n[0-9]{3}$", raw)` group, or `raw`.
fn strip_status(raw: &str) -> &str {
    let tail = |s: &str| -> Option<usize> {
        let b = s.as_bytes();
        let n = b.len();
        (n >= 4 && b[n - 4] == b'\n' && b[n - 3..].iter().all(u8::is_ascii_digit)).then(|| n - 4)
    };
    if let Some(end) = tail(raw) {
        return &raw[..end];
    }
    if let Some(body) = raw.strip_suffix('\n') {
        if let Some(end) = tail(body) {
            return &raw[..end];
        }
    }
    raw
}

/// A key of an object (its last value, as Python keeps).
fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(pairs) => pairs
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v),
        _ => None,
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Option<Vec<String>> {
        note(text.as_bytes()).map(|n| n.lines().to_vec())
    }

    #[test]
    fn the_command_is_the_ask_line() {
        assert_eq!(
            lines(
                r#"{"error":"x","permission":{"command":"k2 permission request --why ...","requestable":true}}"#
            ),
            Some(vec!["Ask with: k2 permission request --why ...".to_string()])
        );
    }

    #[test]
    fn not_requestable_has_the_hint_then_the_line() {
        assert_eq!(
            lines(r#"{"permission":{"requestable":false,"hint":"Only a person can."}}"#),
            Some(vec![
                "Only a person can.".to_string(),
                "It can't be requested.".to_string()
            ])
        );
        assert_eq!(
            lines(r#"{"permission":{"requestable":false}}"#),
            Some(vec!["It can't be requested.".to_string()])
        );
        assert_eq!(
            lines(r#"{"permission":{"requestable":false,"hint":""}}"#),
            Some(vec!["It can't be requested.".to_string()])
        );
        // A hint that isn't a string is left out.
        assert_eq!(
            lines(r#"{"permission":{"requestable":false,"hint":3}}"#),
            Some(vec!["It can't be requested.".to_string()])
        );
        // An empty command falls to the requestable test.
        assert_eq!(
            lines(r#"{"permission":{"command":"","requestable":false}}"#),
            Some(vec!["It can't be requested.".to_string()])
        );
    }

    #[test]
    fn nothing_for_anything_else() {
        for text in [
            r#"{"error":"owner_only"}"#,
            r#"{"permission":null}"#,
            r#"{"permission":"x"}"#,
            r#"{"permission":{}}"#,
            r#"{"permission":{"requestable":0}}"#,
            r#"{"permission":{"command":7}}"#,
            r#"["permission"]"#,
            r#"{"permission":{"command":"x"}"#,
            // Python finds the key, the shell's substring test doesn't.
            concat!(r#"{"\u"#, r#"0070ermission":{"command":"x"}}"#),
            // A lone surrogate: Python can't print it.
            r#"{"permission":{"command":"k2 \ud83d"}}"#,
            r#"{"permission":{"requestable":false,"hint":"\udc00"}}"#,
            "",
        ] {
            assert_eq!(lines(text), None, "{text}");
        }
    }

    #[test]
    fn the_status_tail_is_cut_and_the_last_key_wins() {
        assert_eq!(
            lines("{\"permission\":{\"command\":\"a\"}}\n403"),
            Some(vec!["Ask with: a".to_string()])
        );
        assert_eq!(
            lines("{\"permission\":{\"command\":\"a\"}}\n403\n"),
            Some(vec!["Ask with: a".to_string()])
        );
        assert_eq!(lines("{\"permission\":{\"command\":\"a\"}}\n40"), None);
        assert_eq!(
            lines(r#"{"permission":{"command":"a"},"permission":{"command":"b"}}"#),
            Some(vec!["Ask with: b".to_string()])
        );
    }

    #[test]
    fn the_text_is_what_bash_prints() {
        let n = note(br#"{"permission":{"command":"k2 x\n\n"}}"#).unwrap();
        assert_eq!(n.text(), "Ask with: k2 x\n");
        let n = note(br#"{"permission":{"requestable":false,"hint":"a\nb"}}"#).unwrap();
        assert_eq!(n.text(), "a\nb\nIt can't be requested.\n");
        let n = note(br#"{"permission":{"command":"a\u0000b"}}"#).unwrap();
        assert_eq!(n.text(), "Ask with: ab\n");
        let mut out = Vec::new();
        n.print(&mut out).unwrap();
        assert_eq!(out, b"Ask with: ab\n");
    }
}
