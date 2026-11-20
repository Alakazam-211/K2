//! The vault socket and the confirm token, shared by every family that
//! asks a person to approve a step (vault, app, ci, clone, compute,
//! hostchat, hostmail, llm, sources, the API-key verbs).
//!
//! The shell's functions (`cli/src/vault.sh`, `cli/src/job.sh`), and what
//! answers each here:
//!
//! | shell | here |
//! |---|---|
//! | `_vault_sock` | [`sock_path`] |
//! | `_vault_sock_call GET\|POST <route> [json]` | [`sock_call`] |
//! | `_sidecar_send` then `_sidecar_split` | [`sidecar_call`] |
//! | `_vault_exit_for` | [`exit_for`] |
//! | `_vault_fail <status> <body>` | [`fail`] (its line: [`fail_line`]) |
//! | `_vault_person_guard` | [`person_guard`] |
//! | `_with_confirm_token <body> <tok> [field]` | [`with_confirm_token`] |
//! | `tok="$(_vault_confirm_token <action> [id] [label])" \|\| exit 3` | [`confirm_token`] |
//! | `tok="$(_vault_confirm_token_plan <action> <plan> <key>)" \|\| exit 3` | [`confirm_token_plan`] |
//!
//! # The two channels
//!
//! An agent's verbs go through `_sidecar_send` (the cell socket with the
//! passport, else TCP). The steps that are for people only (a confirm, a
//! reveal, the recovery key, the vault password, key operations) go to
//! `<K2 home>/vault.sock` from a terminal outside K2, with no credential
//! and an `X-K2-Tty` header ([`tty`]). Both hold the answer as the shell's
//! `_SC_STATUS` and `_SC_BODY` ([`Split`]).
//!
//! # The token never leaves the process
//!
//! The shell hands the token to Python on stdin, never on argv or in the
//! environment. Here it never leaves this process: it goes from the
//! answer's body into the next request's body. The password typed for a
//! confirm likewise ([`crate::prompt::Secret`]).
//!
//! # Confirm runs in `$(…)`
//!
//! The shell runs `_vault_confirm_token` in a command substitution, so a
//! failure inside it (its `_vault_fail`'s exit 2, 4 or 1, a missing
//! method's exit 3) ends only that subshell, and the caller's `|| exit 3`
//! ends the command with 3 whatever the code was. [`confirm_token`]
//! returns `None` for all of them; the caller's exit is 3. A refusal's
//! closing line is noted in the caller's [`Closing`] all the same: the
//! subshell writes the note where the main shell's EXIT trap reads it.

use crate::exit::Exit;
use crate::families::{get, without_nuls, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::prompt::{self, Secret};
use crate::transport::client::TRIES;
use crate::transport::{permission, Channel, Client, Error, Response};
use std::path::{Path, PathBuf};

/// `_vault_sock`: `${K2_HOME}/vault.sock`.
pub fn sock_path(k2_home: &Path) -> PathBuf {
    k2_home.join("vault.sock")
}

/// `_vault_sock_call`'s `X-K2-Tty`: stdin and stdout, or stdin and
/// stderr, are terminals.
pub fn tty() -> bool {
    prompt::stdin_is_tty() && (prompt::stdout_is_tty() || prompt::stderr_is_tty())
}

/// `_sidecar_split`'s `_SC_STATUS` and `_SC_BODY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// Three digits; `000` when nothing answered.
    pub status: String,
    /// What `$(…)` held before the status line: NULs dropped, and a
    /// `\n000` in front for each refused connect `_k2_curl` tried again
    /// (curl's `-w` line of each failed try).
    pub body: Vec<u8>,
    /// The channel the answer came on, when one came.
    pub channel: Channel,
}

impl Split {
    pub fn ok(&self) -> bool {
        self.status == "200"
    }

    /// The body as a [`Response`], for [`Closing::answer`].
    pub fn response(&self) -> Response {
        Response {
            status: self.status.parse().unwrap_or(0),
            permission: permission::note(&self.body),
            body: self.body.clone(),
            channel: self.channel,
            server_version: None,
        }
    }
}

/// `_vault_sock_call METHOD <route> [json]`: `out=$(curl … -w '\n%{http_code}'
/// 2>/dev/null) || out=""`, then `_sidecar_split`. Any failure is status
/// `000` with an empty body. Never ends the command.
pub fn sock_call(client: &mut Client, sock: &Path, post: bool, route: &str, payload: &[u8]) -> Split {
    match client.vault_sock(sock, post, route, payload, tty()) {
        Ok(r) => answered(client, &r),
        Err(_) => Split {
            status: "000".into(),
            body: Vec::new(),
            channel: Channel::Uds,
        },
    }
}

/// `_sidecar_split "$(_sidecar_send METHOD <path> <payload>)"`. A POST the
/// daemon may have run, or one the session may not send as the owner, is
/// the transport's line and exit 1 (`_k2_end_command 1` from inside the
/// `$(…)`): `Err` with that exit, the line printed.
pub fn sidecar_call(client: &mut Client, post: bool, path: &str, payload: &str) -> Result<Split, Exit> {
    let r = if post {
        client.sidecar_post(path, payload)
    } else {
        client.sidecar_get(path, payload)
    };
    match r {
        Ok(r) => Ok(answered(client, &r)),
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            Err(Exit::Error)
        }
        // `_k2_curl … || true`: every try's `-w` line, the last one the
        // status.
        Err(Error::Unreachable { .. }) => Ok(Split {
            status: "000".into(),
            body: "\n000".repeat(TRIES as usize - 1).into_bytes(),
            channel: Channel::Tcp,
        }),
        Err(_) => Ok(Split {
            status: "000".into(),
            body: Vec::new(),
            channel: Channel::Tcp,
        }),
    }
}

fn answered(client: &Client, r: &Response) -> Split {
    let mut body = "\n000"
        .repeat(client.refused_before_answer() as usize)
        .into_bytes();
    body.extend(without_nuls(&r.body));
    Split {
        status: format!("{:03}", r.status),
        body,
        channel: r.channel,
    }
}

/// `_vault_exit_for`.
pub fn exit_for(status: &str) -> Exit {
    match status {
        "200" => Exit::Ok,
        "400" | "413" | "422" => Exit::Usage,
        "401" | "403" => Exit::Refused,
        "404" => Exit::NotFound,
        // A waited-on card closed without a value (card_dismissed,
        // card_withdrawn): 5, the code `k2 study errors` gives a conflict.
        "410" => Exit::Conflict,
        _ => Exit::Error,
    }
}

/// `_vault_fail`'s Python on the body: `code: hint` for an error object,
/// the error as text otherwise, or the first 300 characters of the body
/// (`the daemon did not answer` for none) when it is not a JSON object.
pub fn fail_line(body: &[u8]) -> String {
    let raw = String::from_utf8_lossy(body);
    let head = || {
        let h: String = raw.chars().take(300).collect();
        if h.is_empty() {
            "the daemon did not answer".to_string()
        } else {
            h
        }
    };
    let Ok(Value::Object(pairs)) = json::parse(&raw) else {
        return head();
    };
    let e = get(&pairs, "error").filter(|e| py::truthy(e));
    match e {
        None => "error: ".to_string(),
        Some(Value::Object(e)) => format!(
            "{}: {}",
            get(e, "code").map_or_else(|| "error".into(), py::str_of),
            get(e, "hint").map_or_else(String::new, py::str_of)
        ),
        Some(e) => py::str_of(e),
    }
}

/// `_vault_fail <status> <body>`: its line on stderr, the refusal's
/// closing line noted for the EXIT trap, and the exit for the status.
pub fn fail(split: &Split, closing: &mut Closing, passport: bool) -> Exit {
    out::stderr_line(&fail_line(&split.body));
    closing.answer(&split.response(), passport);
    exit_for(&split.status)
}

/// `_envn_any HOOK_SOCK HOOK_TOKEN`: inside a K2 terminal.
pub fn in_k2_terminal() -> bool {
    [k2_client::env_names::HOOK_SOCK, k2_client::env_names::HOOK_TOKEN]
        .iter()
        .any(|names| {
            k2_client::env_names::first_nonempty_os_in(names, |n| std::env::var_os(n)).is_some()
        })
}

/// `_vault_person_guard`'s first refusal: a K2 terminal.
pub const OWNER_ONLY: [&str; 2] = [
    "owner_only: this is for a person, from your own terminal or SSH login, not a K2 terminal.",
    "Agents: don't run it; tell your owner.",
];

/// `_vault_person_guard`: refuse inside a K2 terminal (exit 3), and when
/// the vault socket isn't a socket (exit 1). Each prints the shell's
/// lines.
pub fn person_guard(sock: &Path) -> Result<(), Exit> {
    if in_k2_terminal() {
        for l in OWNER_ONLY {
            out::stderr_line(l);
        }
        return Err(Exit::Refused);
    }
    if !crate::families::live_socket(Some(sock)) {
        out::stderr_line(&format!(
            "vault_unavailable: {} isn't there (is the K2 daemon running on this machine?)",
            sock.display()
        ));
        return Err(Exit::Error);
    }
    Ok(())
}

/// `json.dumps(o)`: Python's default separators, ASCII only.
pub fn dumps(v: &Value) -> String {
    // Only a lone surrogate with ensure_ascii off can fail; it is on.
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// An object from its pairs.
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// A string value, or `null` for an empty one (Python's `x or None`).
pub fn str_or_null(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::Str(Text::from(s))
    }
}

/// A string value from bytes a person typed: `sys.stdin.read()` of them,
/// as Python decodes stdin (UTF-8; a byte that isn't is kept as its
/// surrogate escape, which `json.dumps` writes as `\udcXX`).
pub fn text_of_bytes(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => Value::Str(Text::from(s)),
        Err(_) => {
            let mut text = String::from("\"");
            for chunk in b.utf8_chunks() {
                for c in chunk.valid().chars() {
                    match c {
                        '"' => text.push_str("\\\""),
                        '\\' => text.push_str("\\\\"),
                        c if (c as u32) < 0x20 => text.push_str(&format!("\\u{:04x}", c as u32)),
                        c => text.push(c),
                    }
                }
                for byte in chunk.invalid() {
                    text.push_str(&format!("\\udc{byte:02x}"));
                }
            }
            text.push('"');
            json::parse(&text).unwrap_or(Value::Null)
        }
    }
}

/// `_with_confirm_token <body> <tok> [field]`: the body (`{}` when empty)
/// with `field` (default `confirmToken`) set to the token, or `null` for
/// an empty one; an existing field keeps its place. `None` when the body
/// isn't a JSON object (the shell's Python raises there; every caller
/// passes an object it made).
pub fn with_confirm_token(body: &str, tok: &str, field: Option<&str>) -> Option<String> {
    let body = if body.is_empty() { "{}" } else { body };
    let Ok(Value::Object(mut pairs)) = json::parse(body) else {
        return None;
    };
    let field = Text::from(field.unwrap_or("confirmToken"));
    // `sys.stdin.readline().rstrip("\n")`: the token's first line.
    let tok = tok.split('\n').next().unwrap_or("");
    let v = str_or_null(tok);
    match pairs.iter_mut().find(|(k, _)| *k == field) {
        Some((_, old)) => *old = v,
        None => pairs.push((field, v)),
    }
    Some(dumps(&Value::Object(pairs)))
}

/// The actions whose check is this computer's own user check, not the
/// vault password: the confirm says what is approved first.
pub const FIRST_SETUP: &[&str] = &[
    "vault-password-set",
    "recovery-setup",
    "backup-export-initial",
    "box-move-export",
    "backup-key-add",
    "owner-login",
];

pub const NO_METHOD: &str =
    "no_confirm_method: set a vault password first: k2 vault confirm-password set";

pub const NO_PASSWORD_FOR_METHOD: &str = "no_confirm_method: K2_VAULT_CONFIRM_METHOD=vault-password, but no vault password is set (k2 vault confirm-password set)";

/// One field of a 200's JSON, as the shell's one-line Pythons read it
/// (`json.load(sys.stdin).get(key)`): `Err` with the traceback's last
/// line when the body isn't a JSON object.
fn field<'a>(v: &'a Result<Value, Raise>, key: &str) -> Result<Option<&'a Value>, Raise> {
    match v {
        Ok(Value::Object(pairs)) => Ok(get(pairs, key)),
        Ok(other) => Err(Raise::no_attribute(1, other, "get")),
        Err(r) => Err(r.clone()),
    }
}

/// `json.load(sys.stdin)` on a body, as `python3 -c` raises on one that
/// isn't JSON.
pub fn load(body: &[u8]) -> Result<Value, Raise> {
    let raw = String::from_utf8_lossy(body);
    json::parse(&raw).map_err(|e| {
        let why = if raw.trim_matches(crate::families::py_space).is_empty() {
            "Expecting value: line 1 column 1 (char 0)".to_string()
        } else {
            e.to_string()
        };
        Raise::at(1, format!("json.decoder.JSONDecodeError: {why}"))
    })
}

/// `" ".join(x or [])` / `x or ""`, as the shell's one-liners print a
/// field, or the traceback (printed here) and an empty value.
fn text_field(v: &Result<Value, Raise>, key: &str, join: bool) -> String {
    match field(v, key) {
        Ok(Some(x)) if py::truthy(x) => match (join, x) {
            (true, Value::Array(items)) => items
                .iter()
                .map(py::str_of)
                .collect::<Vec<_>>()
                .join(" "),
            (true, other) => match crate::families::py_iter(other, 1) {
                Ok(items) => items.iter().map(py::str_of).collect::<Vec<_>>().join(" "),
                Err(r) => {
                    out::stderr_line(r.text().trim_end_matches('\n'));
                    String::new()
                }
            },
            (false, x) => py::str_of(x),
        },
        Ok(_) => String::new(),
        Err(r) => {
            out::stderr_line(r.text().trim_end_matches('\n'));
            String::new()
        }
    }
}

/// `print(json.load(sys.stdin)["token"])`: the token, or the traceback
/// (printed) and `None`.
fn token_of(body: &[u8]) -> Option<String> {
    let v = load(body);
    let r = match &v {
        Ok(Value::Object(pairs)) => match get(pairs, "token") {
            Some(t) => return Some(py::str_of(t)),
            None => Raise::at(1, "KeyError: 'token'"),
        },
        Ok(Value::Array(_)) => Raise::at(
            1,
            "TypeError: list indices must be integers or slices, not str",
        ),
        Ok(Value::Str(_)) => Raise::at(1, "TypeError: string indices must be integers, not 'str'"),
        Ok(other) => Raise::at(
            1,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        ),
        Err(r) => r.clone(),
    };
    out::stderr_line(r.text().trim_end_matches('\n'));
    None
}

/// What a confirm needs: the client, the vault socket, the caller's
/// EXIT-trap lines and whether the caller has a passport (for them).
pub struct Confirm<'a> {
    pub client: &'a mut Client,
    pub sock: &'a Path,
    pub closing: &'a mut Closing,
    pub passport: bool,
}

impl Confirm<'_> {
    fn post(&mut self, route: &str, body: &[u8]) -> Result<Split, ()> {
        let s = sock_call(self.client, self.sock, true, route, body);
        if s.ok() {
            Ok(s)
        } else {
            // In the `$(…)`: `_vault_fail` ends the subshell; the caller's
            // `|| exit 3` ends the command.
            fail(&s, self.closing, self.passport);
            Err(())
        }
    }
}

/// `tok="$(_vault_confirm_token <action> <secret-id> <label>)" || exit 3`:
/// ask the daemon over the vault socket for a confirm token (Touch ID on
/// a Mac, Windows Hello, this terminal on a server with no owner login, an
/// owner login's password, or the vault password typed here). `None`: the
/// caller ends the command with exit 3 (see the module's notes); what the
/// shell prints on the way is printed.
pub fn confirm_token(c: &mut Confirm<'_>, action: &str, secret_id: &str, label: &str) -> Option<String> {
    let start = dumps(&object(vec![
        ("action", Value::Str(Text::from(action))),
        ("secretId", str_or_null(secret_id)),
    ]));
    let s = c.post("confirm/start", start.as_bytes()).ok()?;
    let v = load(&s.body);
    let mut methods = text_field(&v, "methods", true);
    let mut login = text_field(&v, "login", false);
    let first_setup = FIRST_SETUP.contains(&action);
    let text = text_field(&v, "confirmText", false);
    if first_setup && !text.is_empty() {
        out::stderr_line(&format!("You are approving: {text}"));
    }
    let forced = k2_client::env_names::first_nonempty_os_in(
        k2_client::env_names::VAULT_CONFIRM_METHOD,
        |n| std::env::var_os(n),
    )
    .is_some_and(|(_, v)| v == "vault-password");
    if !first_setup && forced {
        if has_word(&methods, "vault-password") {
            methods = "vault-password".into();
        } else {
            out::stderr_line(NO_PASSWORD_FOR_METHOD);
            return None;
        }
    }
    let id = str_or_null(secret_id);
    let a = || Value::Str(Text::from(action));
    let finish = if has_word(&methods, "local-auth") {
        out::stderr_line("Approve on this Mac (Touch ID or your login password)…");
        object(vec![
            ("action", a()),
            ("secretId", id),
            ("method", Value::Str(Text::from("local-auth"))),
            ("label", Value::Str(Text::from(label))),
        ])
    } else if has_word(&methods, "windows-hello") {
        out::stderr_line("Approve on this PC (Windows Hello or your Windows password)…");
        object(vec![
            ("action", a()),
            ("secretId", id),
            ("method", Value::Str(Text::from("windows-hello"))),
        ])
    } else if has_word(&methods, "terminal") {
        out::stderr_line("Confirming from this terminal (this server has no owner login yet)…");
        object(vec![
            ("action", a()),
            ("secretId", id),
            ("method", Value::Str(Text::from("terminal"))),
        ])
    } else if has_word(&methods, "owner-login-password") {
        if login.is_empty() {
            let typed = prompt::ask("Which owner login on this server (k2 users lists them): ");
            login = String::from_utf8_lossy(typed.expose()).into_owned();
        }
        let pw = prompt::ask_hidden(&format!("Sign-in password for owner login {login}: "));
        object(vec![
            ("action", a()),
            ("secretId", id),
            ("method", Value::Str(Text::from("owner-login-password"))),
            ("username", Value::Str(Text::from(login.as_str()))),
            ("password", text_of_bytes(pw.expose())),
        ])
    } else if has_word(&methods, "vault-password") {
        let pw = prompt::ask_hidden("Vault password: ");
        object(vec![
            ("action", a()),
            ("secretId", id),
            ("method", Value::Str(Text::from("vault-password"))),
            ("password", text_of_bytes(pw.expose())),
        ])
    } else {
        out::stderr_line(NO_METHOD);
        return None;
    };
    let body = Secret::new(dumps(&finish).into_bytes());
    drop(finish);
    let s = c.post("confirm/finish", body.expose()).ok()?;
    drop(body);
    token_of(&s.body)
}

/// `tok="$(_vault_confirm_token_plan <action> <plan> <key>)" || exit 3`:
/// a token bound to a plan the daemon rebuilds and words itself (the
/// `systemdCred` or `rollbackPlan` key holds `plan`, a JSON text). Only
/// the vault password or the K2 Connect password, typed here. `None`: the
/// caller ends the command with exit 3.
pub fn confirm_token_plan(c: &mut Confirm<'_>, action: &str, plan: &str, key: &str) -> Option<String> {
    let plan_v = match json::parse(plan) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let start = dumps(&object(vec![
        ("action", Value::Str(Text::from(action))),
        (key, plan_v.clone()),
    ]));
    let s = c.post("confirm/start", start.as_bytes()).ok()?;
    let v = load(&s.body);
    let methods = text_field(&v, "methods", true);
    let text = text_field(&v, "confirmText", false);
    if !text.is_empty() {
        out::stderr_line(&format!("You are approving: {text}"));
    }
    let method = if has_word(&methods, "vault-password") {
        prompt::prompt("Vault password: ");
        "vault-password"
    } else if has_word(&methods, "connect-password") {
        prompt::prompt("Your K2 Connect password: ");
        "connect-password"
    } else {
        out::stderr_line(NO_METHOD);
        return None;
    };
    let pw = prompt::read_line(prompt::Echo::Hidden, prompt::AtEof::Drop);
    prompt::prompt("\n");
    let finish = object(vec![
        ("action", Value::Str(Text::from(action))),
        ("method", Value::Str(Text::from(method))),
        ("password", text_of_bytes(pw.expose())),
        (key, plan_v),
    ]);
    drop(pw);
    let body = Secret::new(dumps(&finish).into_bytes());
    drop(finish);
    let s = c.post("confirm/finish", body.expose()).ok()?;
    drop(body);
    token_of(&s.body)
}

/// `case " $methods " in *" <word> "*)`.
fn has_word(methods: &str, word: &str) -> bool {
    format!(" {methods} ").contains(&format!(" {word} "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_goes_in_the_body() {
        assert_eq!(
            with_confirm_token(r#"{"name": "A"}"#, "tok1", None).unwrap(),
            r#"{"name": "A", "confirmToken": "tok1"}"#
        );
        // An empty body is {}; an empty token is null.
        assert_eq!(
            with_confirm_token("", "", None).unwrap(),
            r#"{"confirmToken": null}"#
        );
        // Another field, and one already there keeps its place.
        assert_eq!(
            with_confirm_token(r#"{"t": 1, "x": "é"}"#, "z", Some("t")).unwrap(),
            r#"{"t": "z", "x": "é"}"#
        );
        // Only the token's first line.
        assert_eq!(
            with_confirm_token("{}", "a\nb", None).unwrap(),
            r#"{"confirmToken": "a"}"#
        );
        assert_eq!(with_confirm_token("[1]", "a", None), None);
    }

    #[test]
    fn the_exit_for_each_status() {
        for (s, e) in [
            ("200", Exit::Ok),
            ("400", Exit::Usage),
            ("413", Exit::Usage),
            ("422", Exit::Usage),
            ("401", Exit::Refused),
            ("403", Exit::Refused),
            ("404", Exit::NotFound),
            ("410", Exit::Conflict),
            ("409", Exit::Error),
            ("000", Exit::Error),
        ] {
            assert_eq!(exit_for(s), e, "{s}");
        }
        assert_eq!(Exit::Conflict.code(), 5);
    }

    #[test]
    fn the_fail_line_is_the_shell_s() {
        assert_eq!(
            fail_line(br#"{"error":{"code":"locked","hint":"open it"}}"#),
            "locked: open it"
        );
        assert_eq!(fail_line(br#"{"error":{"hint":"h"}}"#), "error: h");
        assert_eq!(fail_line(br#"{"error":{"code":7}}"#), "7: ");
        assert_eq!(fail_line(br#"{"error":"plain"}"#), "plain");
        assert_eq!(fail_line(br#"{"error":null}"#), "error: ");
        assert_eq!(fail_line(br#"{"ok":true}"#), "error: ");
        assert_eq!(fail_line(b"[1]"), "[1]");
        assert_eq!(fail_line(b"not json"), "not json");
        assert_eq!(fail_line(b""), "the daemon did not answer");
        let long = "é".repeat(400);
        assert_eq!(fail_line(long.as_bytes()), "é".repeat(300));
    }

    #[test]
    fn a_method_is_a_whole_word() {
        assert!(has_word("local-auth vault-password", "vault-password"));
        assert!(!has_word("vault-passwords", "vault-password"));
        assert!(!has_word("", "terminal"));
    }

    #[test]
    fn typed_bytes_that_are_not_utf8_keep_their_escape() {
        assert_eq!(dumps(&text_of_bytes(b"p\xffw\"")), r#""p\udcffw\"""#);
        assert_eq!(dumps(&text_of_bytes("pé".as_bytes())), r#""pé""#);
    }

    /// The texts are the shell's.
    #[test]
    fn the_lines_are_the_shell_s() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/vault.sh"),
        )
        .unwrap();
        for l in OWNER_ONLY
            .iter()
            .chain([NO_METHOD, NO_PASSWORD_FOR_METHOD].iter())
            .chain(
                [
                    "Approve on this Mac (Touch ID or your login password)…",
                    "Approve on this PC (Windows Hello or your Windows password)…",
                    "Confirming from this terminal (this server has no owner login yet)…",
                    "Which owner login on this server (k2 users lists them): ",
                    "Sign-in password for owner login %s: ",
                    "printf 'Vault password: '",
                    "isn't there (is the K2 daemon running on this machine?)",
                ]
                .iter(),
            )
        {
            assert!(src.contains(l), "not in vault.sh: {l}");
        }
        for action in FIRST_SETUP {
            assert!(src.contains(action), "{action}");
        }
        let job = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/job.sh"),
        )
        .unwrap();
        assert!(job.contains("printf 'Your K2 Connect password: '"));
    }
}
