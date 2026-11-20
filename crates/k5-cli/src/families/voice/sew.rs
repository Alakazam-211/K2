//! What `k5 voice`, `k5 hostchat` and `k5 chat` share: the shell CLI's
//! `_hostchat_call` (`cli/src/hostchat-voice.sh`) and the compute helpers
//! it stands on (`cli/src/compute-lib.sh`): `_compute_send`,
//! `_compute_split`, `_compute_fail` and `_compute_json`.
//!
//! Written inside this lane's families for now (the brief's rule for a
//! helper not on the line yet): other lanes port the same helpers for
//! compute, app and board, and the gardener folds them into one.
//!
//! - [`send`]: `raw="$(_compute_send …)"`, split as `_compute_split` splits
//!   it ([`Sent`]).
//! - [`call`]: `_hostchat_call`: the send, the refusal noted for the exit
//!   trap ([`Closing`]), and any status but 200 ends the command with
//!   `_compute_fail`'s line ([`fail_line`]): exit 2 for 400 and 413, else 3.
//! - [`json`]: `_compute_json K2_CJ_<key>=<value>…`, the request bodies.
//!
//! The shell's `_compute_fail` and `_compute_json` read three things from
//! the environment that no caller here sets: `_CP_404_EXIT` (a known bash
//! bug, fixed on its own), `_K2_REFUSAL_AREA` and any `K2_CJ_` variable.
//! A family hands a command line to the shell when one of them is set
//! ([`env_shell`]).

use crate::exit::Exit;
use crate::families::without_nuls;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::permission;
use crate::transport::{Channel, Client, Error, Response};
use std::ffi::OsString;

/// What `raw="$(_compute_send …)"` holds, split as `_compute_split`
/// splits it: the status after the last newline, the body before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub status: String,
    pub body: Vec<u8>,
}

impl Sent {
    /// `_CP_STATUS="${raw##*$'\n'}"; _CP_BODY="${raw%$'\n'*}"` (the body
    /// empty when there is no newline), `000` for an empty status, with
    /// `$(…)`'s NULs and trailing newlines gone first.
    pub fn split(raw: &[u8]) -> Sent {
        let mut raw = without_nuls(raw);
        while raw.last() == Some(&b'\n') {
            raw.pop();
        }
        let (status, body) = match raw.iter().rposition(|b| *b == b'\n') {
            Some(at) => (raw[at + 1..].to_vec(), raw[..at].to_vec()),
            None => (raw.clone(), Vec::new()),
        };
        let status = String::from_utf8_lossy(&status).into_owned();
        Sent {
            status: if status.is_empty() {
                "000".into()
            } else {
                status
            },
            body,
        }
    }
}

/// `-w '\n%{http_code}'` of a try that got no answer.
const NO_STATUS: &[u8] = b"\n000";

/// `_compute_send <method> <path> <payload> [tcp]` inside `$(…)`: what it
/// printed, split; or the exit, after its line, when it ended the command
/// (a POST on the cell socket with no answer, or no passport in a
/// session).
pub fn send(
    client: &mut Client,
    post: bool,
    path: &str,
    payload: &str,
    tcp: bool,
) -> Result<Sent, Exit> {
    let result = client.compute_send(post, path, payload, tcp);
    // Each refused try curl made before the last wrote its own status.
    let mut raw = NO_STATUS.repeat(client.refused_before_answer() as usize);
    match result {
        Ok(r) => {
            raw.extend_from_slice(&r.body);
            raw.extend_from_slice(format!("\n{:03}", r.status).as_bytes());
        }
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            return Err(Exit::Error);
        }
        // The last try's status, then `|| printf '\n000'`.
        Err(_) => {
            raw.extend_from_slice(NO_STATUS);
            raw.extend_from_slice(NO_STATUS);
        }
    }
    Ok(Sent::split(&raw))
}

/// `_permission_note "$_CP_BODY"` (in `_compute_split`): the body's
/// closing line, bulletin and refused passport, for the exit trap.
pub fn note(closing: &mut Closing, body: &[u8], passport: bool) {
    let r = Response {
        status: 0,
        body: body.to_vec(),
        channel: Channel::Tcp,
        permission: permission::note(body),
        server_version: None,
    };
    closing.answer(&r, passport);
}

/// `_hostchat_call <method> <path> <payload> [tcp]`: the body of a 200;
/// anything else is `_compute_fail`'s line and its exit.
pub fn call(
    client: &mut Client,
    closing: &mut Closing,
    post: bool,
    path: &str,
    payload: &str,
    tcp: bool,
) -> Result<Vec<u8>, Exit> {
    let sent = send(client, post, path, payload, tcp)?;
    note(closing, &sent.body, client.passport().is_some());
    if sent.status == "200" {
        return Ok(sent.body);
    }
    let (line, exit) = fail_line(&sent.status, &sent.body);
    out::stderr_line(&line);
    Err(exit)
}

/// `_compute_fail <status> <body>` (no `_K2_REFUSAL_AREA`): the line it
/// echoes and its exit.
pub fn fail_line(status: &str, body: &[u8]) -> (String, Exit) {
    let exit = match status {
        "400" | "413" => Exit::Usage,
        _ => Exit::Refused,
    };
    let line = if body.is_empty() {
        String::new()
    } else {
        fail_python(&String::from_utf8_lossy(body))
    };
    let line = if line.is_empty() {
        format!(
            concat!(
                "{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the ",
                k2_client::product_name!(),
                " daemon (status {})\"}}"
            ),
            status
        )
    } else {
        line
    };
    (line, exit)
}

/// `_compute_fail`'s Python on `raw`, as `line="$(…)"` holds what it
/// printed (trailing newlines gone).
fn fail_python(raw: &str) -> String {
    let mut text = match json::parse(raw) {
        Err(_) => format!(
            "daemon_error: {}",
            raw.chars().take(200).collect::<String>()
        ),
        Ok(v) => {
            let e = match &v {
                Value::Object(pairs) => crate::families::get(pairs, "error"),
                _ => None,
            };
            let message = match &v {
                Value::Object(pairs) => crate::families::get(pairs, "message"),
                _ => None,
            };
            let (code, hint) = match (e, message) {
                (Some(Value::Object(pairs)), _) => (
                    crate::families::get(pairs, "code")
                        .cloned()
                        .unwrap_or_else(|| s("error")),
                    crate::families::get(pairs, "hint")
                        .cloned()
                        .unwrap_or_else(|| s("")),
                ),
                (Some(e @ Value::Str(_)), Some(m @ Value::Str(_))) => (e.clone(), m.clone()),
                (e, _) => (
                    s("error"),
                    s(&e.map_or_else(|| "None".to_string(), py::str_of)),
                ),
            };
            dumps_default(&obj(vec![("error", code), ("hint", hint)]))
        }
    };
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// `json.dumps(v)`: `, ` and `: `, `ensure_ascii`.
pub fn dumps_default(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// One `K2_CJ_<key>=<value>` of `_compute_json`, by the key's ending.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// A string.
    S(String),
    /// `_n`: digits here, read as an integer.
    N(String),
    /// `_b`: `1` is true, anything else false.
    B(String),
    /// `_j`: JSON text.
    J(String),
    /// A string as Python read it from the environment: a value whose
    /// bytes need not be UTF-8 ([`env_text`]). Left out when empty.
    V(Value),
}

/// `_compute_json`: the fields in order, an empty value left out, as
/// `json.dumps` writes them. `None` when a `_j` value is not JSON (the
/// shell's Python raises there; no caller here passes one).
pub fn json(fields: &[(&str, Field)]) -> Option<String> {
    let mut pairs = Vec::new();
    for (key, f) in fields {
        let v = match f {
            Field::S(t) | Field::N(t) | Field::B(t) | Field::J(t) if t.is_empty() => continue,
            Field::V(v) if is_empty_text(v) => continue,
            Field::V(v) => v.clone(),
            Field::S(t) => s(t),
            Field::N(t) => {
                let digits = t.trim_start_matches('0');
                Value::Int(if digits.is_empty() {
                    "0".into()
                } else {
                    digits.into()
                })
            }
            Field::B(t) => Value::Bool(t == "1"),
            Field::J(t) => json::parse(t).ok()?,
        };
        pairs.push((*key, v));
    }
    Some(dumps_default(&obj(pairs)))
}

fn is_empty_text(v: &Value) -> bool {
    matches!(v, Value::Str(t) if t.code_points().is_empty())
}

/// What Python's `os.environ` holds for a variable whose value is `bytes`
/// (UTF-8, `surrogateescape`): each byte that is not part of good UTF-8
/// stands for itself as a lone surrogate, U+DC80 to U+DCFF, which
/// `json.dumps` writes as `\udcXX`.
pub fn env_text(bytes: &[u8]) -> Value {
    let mut lit = String::from("\"");
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '"' => lit.push_str("\\\""),
                '\\' => lit.push_str("\\\\"),
                c if u32::from(c) < 0x20 => lit.push_str(&format!("\\u{:04x}", u32::from(c))),
                c => lit.push(c),
            }
        }
        for b in chunk.invalid() {
            lit.push_str(&format!("\\udc{b:02x}"));
        }
    }
    lit.push('"');
    json::parse(&lit).unwrap_or_else(|_| s(&String::from_utf8_lossy(bytes)))
}

/// A variable the shell's helpers read from the environment and no caller
/// here sets (see the module's notes): the command is the shell's.
pub fn env_shell(names: impl IntoIterator<Item = OsString>) -> bool {
    names.into_iter().any(|n| {
        n.to_str().is_some_and(|n| {
            n == "_CP_404_EXIT" || n == "_K2_REFUSAL_AREA" || n.starts_with("K2_CJ_")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_as_the_shell_splits() {
        let s = |raw: &[u8], status: &str, body: &[u8]| {
            assert_eq!(
                Sent::split(raw),
                Sent {
                    status: status.into(),
                    body: body.to_vec()
                },
                "{raw:?}"
            )
        };
        s(b"{\"a\":1}\n200", "200", b"{\"a\":1}");
        s(b"x\n\n200\n", "200", b"x\n");
        s(b"\n000\n000", "000", b"\n000");
        s(b"", "000", b"");
        s(b"200", "200", b"");
        s(b"a\0b\n404", "404", b"ab");
    }

    #[test]
    fn fail_lines() {
        let f = |status: &str, body: &str| fail_line(status, body.as_bytes());
        assert_eq!(
            f("403", r#"{"error":{"code":"chat_manage_off","hint":"é"}}"#),
            (
                r#"{"error": "chat_manage_off", "hint": "\u00e9"}"#.into(),
                Exit::Refused
            )
        );
        assert_eq!(
            f("400", r#"{"error":"bad","message":"no"}"#),
            (r#"{"error": "bad", "hint": "no"}"#.into(), Exit::Usage)
        );
        assert_eq!(
            f("413", r#"{"error":["a"]}"#),
            (r#"{"error": "error", "hint": "['a']"}"#.into(), Exit::Usage)
        );
        assert_eq!(
            f("500", "[1]"),
            (
                r#"{"error": "error", "hint": "None"}"#.into(),
                Exit::Refused
            )
        );
        assert_eq!(
            f("502", "<html>\n\n"),
            ("daemon_error: <html>".into(), Exit::Refused)
        );
        assert_eq!(
            f("000", ""),
            (
                format!(
                    "{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the {} daemon (status 000)\"}}",
                    k2_client::brand::PRODUCT_NAME
                ),
                Exit::Refused
            )
        );
    }

    #[test]
    fn json_as_compute_json_writes_it() {
        let j = json(&[
            ("room", Field::S("lounge".into())),
            ("agent", Field::S(String::new())),
            ("count", Field::N("007".into())),
            ("now", Field::B("1".into())),
            ("off", Field::B("0".into())),
            ("required", Field::J("null".into())),
            ("level", Field::J("\"none\"".into())),
        ]);
        assert_eq!(
            j.as_deref(),
            Some(
                r#"{"room": "lounge", "count": 7, "now": true, "off": false, "required": null, "level": "none"}"#
            )
        );
        assert_eq!(json(&[]).as_deref(), Some("{}"));
    }

    #[test]
    fn env_text_as_python_reads_it() {
        let j = |bytes: &[u8]| json(&[("text", Field::V(env_text(bytes)))]);
        assert_eq!(
            j(b"hi \"x\" \\ \t").as_deref(),
            Some(r#"{"text": "hi \"x\" \\ \t"}"#)
        );
        assert_eq!(
            j("\u{e9}\u{1f600}".as_bytes()).as_deref(),
            Some(r#"{"text": "\u00e9\ud83d\ude00"}"#)
        );
        assert_eq!(
            j(b"a\xff\xe2\x82b").as_deref(),
            Some(r#"{"text": "a\udcff\udce2\udc82b"}"#)
        );
        assert_eq!(j(b"").as_deref(), Some("{}"));
    }

    #[test]
    fn the_shell_s_lines() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/compute-lib.sh");
        let text = std::fs::read_to_string(&path).unwrap();
        for shell in [
            r#"    _CP_STATUS="${raw##*$'\n'}""#,
            r#"    _CP_BODY="${raw%$'\n'*}""#,
            r#"    [ "$_CP_STATUS" = "$raw" ] && _CP_BODY="""#,
            r#"    [ -z "$_CP_STATUS" ] && _CP_STATUS="000""#,
            r#"    _permission_note "$_CP_BODY""#,
            r#"    print("daemon_error: " + raw[:200]); sys.exit(0)"#,
            r#"    out = {"error": e.get("code", "error"), "hint": e.get("hint", "")}"#,
            r#"elif isinstance(e, str) and isinstance(v.get("message"), str):"#,
            r#"    out = {"error": "error", "hint": str(e)}"#,
            r#"        400|413) exit 2 ;;"#,
            r#"        404) exit "${_CP_404_EXIT:-3}" ;;"#,
            r#"        *) exit 3 ;;"#,
            r#"        out[key[:-2]] = float(v) if "." in v else int(v)"#,
            r#"        out[key[:-2]] = v == "1""#,
        ] {
            assert!(
                text.lines().any(|l| l == shell),
                "compute-lib.sh no longer has {shell:?}"
            );
        }
        assert!(text.contains(
            "    [ -z \"$line\" ] && line=\"{\\\"error\\\":\\\"daemon_unreachable\\\",\\\"hint\\\":\\\"no answer from the K2 daemon (status ${status:-none})\\\"}\"\n"
        ));
    }
}
