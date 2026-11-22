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
use k2_client::brand::PRODUCT_NAME;
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
pub fn sock_call(
    client: &mut Client,
    sock: &Path,
    post: bool,
    route: &str,
    payload: &[u8],
) -> Split {
    match client.vault_sock(sock, post, route, payload, tty()) {
        Ok(r) => answered(client, &r),
        Err(_) => Split {
            status: "000".into(),
            body: Vec::new(),
            channel: Channel::Uds,
        },
    }
}

/// `_migrate_send METHOD <route> [json]`'s vault socket arm
/// (`cli/src/migrate.sh`): [`sock_call`] for
/// `/cli/migrate/secrets/<route>` ([`Client::migrate_sock`]). Any failure
/// is status `000` with an empty body. Never ends the command.
pub fn migrate_sock_call(
    client: &mut Client,
    sock: &Path,
    post: bool,
    route: &str,
    payload: &[u8],
) -> Split {
    match client.migrate_sock(sock, post, route, payload, tty()) {
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
pub fn sidecar_call(
    client: &mut Client,
    post: bool,
    path: &str,
    payload: &str,
) -> Result<Split, Exit> {
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
    [
        k2_client::env_names::HOOK_SOCK,
        k2_client::env_names::HOOK_TOKEN,
    ]
    .iter()
    .any(|names| {
        k2_client::env_names::first_nonempty_os_in(names, |n| std::env::var_os(n)).is_some()
    })
}

/// `_vault_person_guard`'s first refusal: a K2 terminal.
pub const OWNER_ONLY: [&str; 2] = [
    concat!(
        "owner_only: this is for a person, from your own terminal or SSH login, not a ",
        k2_client::product_name!(),
        " terminal."
    ),
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
            "vault_unavailable: {} isn't there (is the {PRODUCT_NAME} daemon running on this machine?)",
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

pub const NO_METHOD: &str = concat!(
    "no_confirm_method: set a vault password first: ",
    k2_client::cli_name!(),
    " vault confirm-password set"
);

/// The setting's name is the one the shell reads (a stored name, not the
/// brand's).
pub const NO_PASSWORD_FOR_METHOD: &str = concat!(
    "no_confirm_method: K2_VAULT_CONFIRM_METHOD=vault-password, but no vault password is set (",
    k2_client::cli_name!(),
    " vault confirm-password set)"
);

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
            (true, Value::Array(items)) => {
                items.iter().map(py::str_of).collect::<Vec<_>>().join(" ")
            }
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
/// owner login's password, the vault password typed here, or the owner's
/// phone). `None`: the caller ends the command with exit 3 (see the
/// module's notes); what the shell prints on the way is printed.
pub fn confirm_token(
    c: &mut Confirm<'_>,
    action: &str,
    secret_id: &str,
    label: &str,
) -> Option<String> {
    confirm_token_with(c, action, secret_id, label, None)
}

/// `_vault_confirm_token`'s phone method and timeouts.
const PHONE_WAIT: std::time::Duration = std::time::Duration::from_secs(330);

/// `_vault_confirm_token <action> <secret-id> <label> <extra>`: as
/// [`confirm_token`], with `extra` (a JSON object: a share's `grant`)
/// merged into the phone card's request.
///
/// The phone (0.46.17), when the start's answer offers it
/// (`phoneOffered`): `K2_VAULT_CONFIRM_METHOD=phone`, or `phoneOnly` with
/// no method set, asks the phone alone; over SSH the phone card goes up
/// beside this computer's own prompt (Touch ID, Windows Hello; the first
/// answer wins); and when nobody answers that prompt (409
/// `confirm_unanswered` or `vault_presence_required`) the phone card,
/// once. Any other method set means never the phone.
pub fn confirm_token_with(
    c: &mut Confirm<'_>,
    action: &str,
    secret_id: &str,
    label: &str,
    extra: Option<&Value>,
) -> Option<String> {
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
    let want_m = env_value(k2_client::env_names::VAULT_CONFIRM_METHOD);
    if !first_setup && want_m == "vault-password" {
        if has_word(&methods, "vault-password") {
            methods = "vault-password".into();
        } else {
            out::stderr_line(NO_PASSWORD_FOR_METHOD);
            return None;
        }
    }
    let mut phone_ok = flag_field(&v, "phoneOffered");
    let phone_only = flag_field(&v, "phoneOnly");
    let over_ssh = ["SSH_CONNECTION", "SSH_TTY"]
        .iter()
        .any(|n| std::env::var_os(n).is_some_and(|x| !x.is_empty()));
    if want_m == "phone" || (phone_only && phone_ok && want_m.is_empty()) {
        if !phone_ok {
            out::stderr_line(NO_PHONE_HERE);
            return None;
        }
        methods = "phone".into();
    }
    if !want_m.is_empty() && want_m != "phone" {
        phone_ok = false;
    }
    let mut req = String::new();
    let mut max = None;
    let id = || str_or_null(secret_id);
    let a = || Value::Str(Text::from(action));
    let finish = if has_word(&methods, "phone") {
        let r = phone_raise(c, action, secret_id, label, extra);
        if r.req.is_empty() {
            out::stderr_line(
                "no_phone_confirm: the phone card could not be raised (above)",
            );
            return None;
        }
        out::stderr_line(&format!(
            "Approve on your phone: {}. Waiting for your phone… (5 minutes)",
            r.line
        ));
        max = Some(PHONE_WAIT);
        req = r.req;
        phone_body(action, secret_id, label, &req)
    } else if has_word(&methods, "local-auth") || has_word(&methods, "windows-hello") {
        let (lm, where_word) = if has_word(&methods, "local-auth") {
            ("local-auth", "this Mac (Touch ID or your login password)")
        } else {
            (
                "windows-hello",
                "this PC (Windows Hello or your Windows password)",
            )
        };
        if phone_ok && over_ssh {
            let r = phone_raise(c, action, secret_id, label, extra);
            req = r.req;
            if !req.is_empty() {
                out::stderr_line(&format!(
                    "Approve on {where_word}, or on your phone: {}. Waiting for your phone… (5 minutes; the first answer wins)",
                    r.line
                ));
                if lm == "windows-hello" {
                    out::stderr_line(WINDOWS_HELLO_STAYS);
                }
                max = Some(PHONE_WAIT);
            }
        }
        if req.is_empty() {
            out::stderr_line(&format!("Approve on {where_word}…"));
        }
        let mut o = vec![
            ("action", a()),
            ("secretId", id()),
            ("method", Value::Str(Text::from(lm))),
            ("label", Value::Str(Text::from(label))),
        ];
        if !req.is_empty() {
            o.push(("phoneRequest", Value::Str(Text::from(req.as_str()))));
        }
        object(o)
    } else if has_word(&methods, "terminal") {
        out::stderr_line("Confirming from this terminal (this server has no owner login yet)…");
        object(vec![
            ("action", a()),
            ("secretId", id()),
            ("method", Value::Str(Text::from("terminal"))),
        ])
    } else if has_word(&methods, "owner-login-password") {
        if login.is_empty() {
            let typed = prompt::ask(concat!(
                "Which owner login on this server (",
                k2_client::cli_name!(),
                " users lists them): "
            ));
            login = String::from_utf8_lossy(typed.expose()).into_owned();
        }
        let pw = prompt::ask_hidden(&format!("Sign-in password for owner login {login}: "));
        object(vec![
            ("action", a()),
            ("secretId", id()),
            ("method", Value::Str(Text::from("owner-login-password"))),
            ("username", Value::Str(Text::from(login.as_str()))),
            ("password", text_of_bytes(pw.expose())),
        ])
    } else if has_word(&methods, "vault-password") {
        let pw = prompt::ask_hidden("Vault password: ");
        object(vec![
            ("action", a()),
            ("secretId", id()),
            ("method", Value::Str(Text::from("vault-password"))),
            ("password", text_of_bytes(pw.expose())),
        ])
    } else {
        out::stderr_line(NO_METHOD);
        return None;
    };
    let body = Secret::new(dumps(&finish).into_bytes());
    drop(finish);
    let mut s = sock_within(c, "confirm/finish", body.expose(), max);
    drop(body);
    // Nobody answered this computer's own prompt: the phone, once.
    if s.status == "409" && phone_ok && req.is_empty() && methods != "phone" && unanswered(&s.body)
    {
        out::stderr_line("Nobody answered on this computer.");
        let r = phone_raise(c, action, secret_id, label, extra);
        if r.req.is_empty() {
            // `_vault_fail` on the phone card's own answer: a 200 there
            // (its reader raised) exits 0 with no token.
            return match fail(&r.split, c.closing, c.passport) {
                Exit::Ok => Some(String::new()),
                _ => None,
            };
        }
        out::stderr_line(&format!(
            "Approve on your phone: {}. Waiting for your phone… (5 minutes)",
            r.line
        ));
        let b = dumps(&phone_body(action, secret_id, label, &r.req));
        s = sock_within(c, "confirm/finish", b.as_bytes(), Some(PHONE_WAIT));
    }
    if !s.ok() {
        fail(&s, c.closing, c.passport);
        return None;
    }
    token_after(&s.body)
}

/// The finish's reader: `Confirmed on your phone.` when the phone
/// answered (`confirmedOn`), then the token; or the traceback (printed)
/// and `None`.
fn token_after(body: &[u8]) -> Option<String> {
    let r = match load(body) {
        Ok(Value::Object(pairs)) => {
            if matches!(get(&pairs, "confirmedOn"), Some(Value::Str(t)) if *t == Text::from("phone"))
            {
                out::stderr_line("Confirmed on your phone.");
            }
            match get(&pairs, "token") {
                Some(t) => return Some(py::str_of(t)),
                None => Raise::at(6, "KeyError: 'token'"),
            }
        }
        Ok(other) => Raise::no_attribute(4, &other, "get"),
        Err(r) => Raise::at(3, r.exception),
    };
    out::stderr_line(r.text().trim_end_matches('\n'));
    None
}

/// `no_phone_confirm` when the phone was asked for and isn't offered.
pub const NO_PHONE_HERE: &str = concat!(
    "no_phone_confirm: this step can't be confirmed from a phone here (the owner's own terminal, ",
    "permission cards on, and not an owner-login change); unset K2_VAULT_CONFIRM_METHOD to confirm at this computer"
);

/// Beside the phone card, on a PC.
pub const WINDOWS_HELLO_STAYS: &str =
    "(If you answer on the phone, the Windows Hello prompt on this PC may stay open: dismiss it.)";

/// `_envn NAME`: the first non-empty of its K5_, K2_ and K2SO_ names.
fn env_value(names: &[&'static str]) -> String {
    k2_client::env_names::first_nonempty_os_in(names, |n| std::env::var_os(n))
        .map(|(_, v)| v.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `print("1" if json.load(sys.stdin).get(key) else "")`: the traceback
/// (printed) reads as no.
fn flag_field(v: &Result<Value, Raise>, key: &str) -> bool {
    match field(v, key) {
        Ok(x) => x.is_some_and(py::truthy),
        Err(r) => {
            out::stderr_line(r.text().trim_end_matches('\n'));
            false
        }
    }
}

/// `_vault_sock_call POST <route> <json> <max-time>`.
fn sock_within(
    c: &mut Confirm<'_>,
    route: &str,
    body: &[u8],
    max: Option<std::time::Duration>,
) -> Split {
    c.client.vault_sock_max_time = max;
    let s = sock_call(c.client, c.sock, true, route, body);
    c.client.vault_sock_max_time = None;
    s
}

/// The confirm/finish body that the phone answers.
fn phone_body(action: &str, secret_id: &str, label: &str, req: &str) -> Value {
    object(vec![
        ("action", Value::Str(Text::from(action))),
        ("secretId", str_or_null(secret_id)),
        ("method", Value::Str(Text::from("phone"))),
        ("label", Value::Str(Text::from(label))),
        ("phoneRequest", Value::Str(Text::from(req))),
    ])
}

/// `(json.load(sys.stdin).get("error") or {}).get("code") in
/// ("confirm_unanswered", "vault_presence_required")`: the traceback
/// (printed) reads as no.
fn unanswered(body: &[u8]) -> bool {
    let code = load(body).and_then(|v| {
        let e = crate::families::py_get(&v, "error", 1)?
            .filter(|e| py::truthy(e))
            .cloned()
            .unwrap_or(Value::Object(Vec::new()));
        Ok(crate::families::py_get(&e, "code", 1)?.cloned())
    });
    match code {
        Ok(Some(Value::Str(t))) => {
            t == Text::from("confirm_unanswered") || t == Text::from("vault_presence_required")
        }
        Ok(_) => false,
        Err(r) => {
            out::stderr_line(r.text().trim_end_matches('\n'));
            false
        }
    }
}

/// `_vault_phone_raise`'s `_VPC_REQ` and `_VPC_LINE`, and the answer its
/// start got (the `_SC_STATUS` and `_SC_BODY` a `_vault_fail` after it
/// reads).
struct Raised {
    req: String,
    line: String,
    split: Split,
}

/// `_vault_phone_raise <action> <secret-id> <label> <extra>`: raise the
/// owner's phone card for this confirm, on the Thread of the workspace
/// this terminal is in (`PROJECT_PATH`, else `$PWD`) or
/// `VAULT_CONFIRM_THREAD`'s. A refusal prints why and raises nothing.
fn phone_raise(
    c: &mut Confirm<'_>,
    action: &str,
    secret_id: &str,
    label: &str,
    extra: Option<&Value>,
) -> Raised {
    let mut here = env_value(k2_client::env_names::PROJECT_PATH);
    if here.is_empty() {
        here = crate::daemon::logical_cwd(std::env::var_os("PWD"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    let thread = env_value(k2_client::env_names::VAULT_CONFIRM_THREAD);
    let mut o: Vec<(Text, Value)> = vec![
        (Text::from("action"), Value::Str(Text::from(action))),
        (Text::from("secretId"), str_or_null(secret_id)),
        (Text::from("label"), Value::Str(Text::from(label))),
        (Text::from("phone"), Value::Bool(true)),
        (Text::from("workspacePath"), Value::Str(Text::from(here.as_str()))),
    ];
    if !thread.is_empty() {
        o.push((Text::from("thread"), Value::Str(Text::from(thread.as_str()))));
    }
    // `o.update(extra)`: a key already there keeps its place.
    if let Some(Value::Object(more)) = extra {
        for (k, v) in more {
            match o.iter_mut().find(|(ok, _)| ok == k) {
                Some((_, old)) => *old = v.clone(),
                None => o.push((k.clone(), v.clone())),
            }
        }
    }
    let split = sock_call(
        c.client,
        c.sock,
        true,
        "confirm/start",
        dumps(&Value::Object(o)).as_bytes(),
    );
    let none = |split| Raised {
        req: String::new(),
        line: String::new(),
        split,
    };
    if !split.ok() {
        match no_card_line(&split.body) {
            Ok(l) => out::stderr_line(&l),
            Err(r) => out::stderr_line(r.text().trim_end_matches('\n')),
        }
        return none(split);
    }
    match card_line(&split.body) {
        Ok(l) => {
            let (req, line) = read_tab_pair(&l);
            Raised { req, line, split }
        }
        Err(r) => {
            out::stderr_line(r.text().trim_end_matches('\n'));
            none(split)
        }
    }
}

/// The refusal's line: `(no phone card: <hint, else code, else "the
/// daemon did not answer">)`. A body that isn't a JSON object is no
/// error; an error that is something else than an object raises.
fn no_card_line(body: &[u8]) -> Result<String, Raise> {
    let e = match load(body) {
        Ok(Value::Object(pairs)) => get(&pairs, "error")
            .filter(|e| py::truthy(e))
            .cloned()
            .unwrap_or(Value::Object(Vec::new())),
        _ => Value::Object(Vec::new()),
    };
    let pick = |k: &str| -> Result<Option<Value>, Raise> {
        Ok(crate::families::py_get(&e, k, 7)?
            .filter(|x| py::truthy(x))
            .cloned())
    };
    let why = match pick("hint")? {
        Some(h) => py::str_of(&h),
        None => match pick("code")? {
            Some(c) => py::str_of(&c),
            None => "the daemon did not answer".to_string(),
        },
    };
    Ok(format!("(no phone card: {why})"))
}

/// The card's line, `<request>\tcard <request> on <addr>'s Thread[ —
/// <thread>] (<text>)`, as the shell's Python prints it (up to its first
/// newline, as `read` takes it).
fn card_line(body: &[u8]) -> Result<String, Raise> {
    let v = load(body).map_err(|r| Raise::at(3, r.exception))?;
    let p = crate::families::py_get(&v, "phone", 3)?
        .filter(|p| py::truthy(p))
        .cloned()
        .unwrap_or(Value::Object(Vec::new()));
    let why = match crate::families::py_get(&p, "thread", 4)? {
        Some(x @ Value::Str(_)) if py::truthy(x) => format!(" — {}", py::str_of(x)),
        Some(x) if py::truthy(x) => {
            return Err(Raise::at(
                4,
                format!(
                    "TypeError: can only concatenate str (not \"{}\") to str",
                    py::type_name(x)
                ),
            ))
        }
        _ => String::new(),
    };
    // `p.get(key, "")`: a key there with null is "None".
    let f = |k: &str| match &p {
        Value::Object(pairs) => get(pairs, k).map_or_else(String::new, py::str_of),
        _ => String::new(),
    };
    let req = f("request");
    Ok(format!(
        "{req}\tcard {req} on {}'s Thread{why} ({})",
        f("addr"),
        f("text")
    ))
}

/// `IFS=$'\t' read -r a b`: the first line, tabs at its ends dropped,
/// split at its first tab; the rest, its tabs in front dropped, is `b`.
fn read_tab_pair(out: &str) -> (String, String) {
    let line = out.split('\n').next().unwrap_or("").replace('\0', "");
    let line = line.trim_matches('\t');
    match line.split_once('\t') {
        Some((a, b)) => (a.to_string(), b.trim_start_matches('\t').to_string()),
        None => (line.to_string(), String::new()),
    }
}

/// `tok="$(_vault_confirm_token_plan <action> <plan> <key>)" || exit 3`:
/// a token bound to a plan the daemon rebuilds and words itself (the
/// `systemdCred` or `rollbackPlan` key holds `plan`, a JSON text). Only
/// the vault password or the K2 Connect password, typed here. `None`: the
/// caller ends the command with exit 3.
pub fn confirm_token_plan(
    c: &mut Confirm<'_>,
    action: &str,
    plan: &str,
    key: &str,
) -> Option<String> {
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
        prompt::prompt(concat!(
            "Your ",
            k2_client::product_name!(),
            " Connect password: "
        ));
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
            with_confirm_token("{\"t\": 1, \"x\": \"\u{e9}\"}", "z", Some("t")).unwrap(),
            "{\"t\": \"z\", \"x\": \"\\u00e9\"}"
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
        assert_eq!(dumps(&text_of_bytes("p\u{e9}".as_bytes())), "\"p\\u00e9\"");
    }

    #[test]
    fn read_splits_at_the_first_tab() {
        let r = |s: &str| read_tab_pair(s);
        assert_eq!(r("pr_1\tcard pr_1 on a's Thread (t)\n"), ("pr_1".into(), "card pr_1 on a's Thread (t)".into()));
        // No request: the leading tab goes, and the line is the request.
        assert_eq!(r("\tcard  on a's Thread ()\n"), ("card  on a's Thread ()".into(), String::new()));
        // More tabs stay in the rest; a newline ends the line.
        assert_eq!(r("a\t\tb\tc\t\nnext"), ("a".into(), "b\tc".into()));
        assert_eq!(r(""), (String::new(), String::new()));
    }

    #[test]
    fn the_card_line_is_the_shell_s() {
        assert_eq!(
            card_line(br#"{"phone":{"request":"pr_1","addr":"ws1","thread":"why","text":"Rekey"}}"#).unwrap(),
            "pr_1\tcard pr_1 on ws1's Thread — why (Rekey)"
        );
        // A key there with null prints None; a missing one is empty.
        assert_eq!(
            card_line(br#"{"phone":{"request":null}}"#).unwrap(),
            "None\tcard None on 's Thread ()"
        );
        assert_eq!(card_line(br#"{"phone":0}"#).unwrap(), "\tcard  on 's Thread ()");
        assert!(card_line(br#"{"phone":{"thread":5}}"#)
            .unwrap_err()
            .exception
            .starts_with("TypeError: can only concatenate str (not \"int\")"));
        assert_eq!(card_line(br#"{"phone":"x"}"#).unwrap_err().frames, vec![(4, "<module>")]);
        assert_eq!(card_line(b"[1]").unwrap_err().frames, vec![(3, "<module>")]);
        assert_eq!(card_line(b"").unwrap_err().frames, vec![(3, "<module>")]);
    }

    #[test]
    fn the_no_card_line_is_the_shell_s() {
        assert_eq!(no_card_line(br#"{"error":{"code":"c","hint":"h"}}"#).unwrap(), "(no phone card: h)");
        assert_eq!(no_card_line(br#"{"error":{"code":"c","hint":""}}"#).unwrap(), "(no phone card: c)");
        assert_eq!(no_card_line(b"").unwrap(), "(no phone card: the daemon did not answer)");
        assert_eq!(no_card_line(b"[1]").unwrap(), "(no phone card: the daemon did not answer)");
        assert!(no_card_line(br#"{"error":"boom"}"#).is_err());
    }

    #[test]
    fn only_these_codes_fall_back_to_the_phone() {
        assert!(unanswered(br#"{"error":{"code":"confirm_unanswered"}}"#));
        assert!(unanswered(br#"{"error":{"code":"vault_presence_required"}}"#));
        assert!(!unanswered(br#"{"error":{"code":"locked"}}"#));
        assert!(!unanswered(br#"{"error":null}"#));
        assert!(!unanswered(b"not json"));
        assert!(!unanswered(br#"{"error":"confirm_unanswered"}"#));
    }

    /// The texts are the shell's (in the default brand, whose names the
    /// shell spells).
    #[test]
    fn the_lines_are_the_shell_s() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/vault.sh"),
        )
        .unwrap();
        for l in OWNER_ONLY
            .iter()
            .chain([NO_METHOD, NO_PASSWORD_FOR_METHOD, NO_PHONE_HERE, WINDOWS_HELLO_STAYS].iter())
            .chain(
                [
                    "where_word=\"this Mac (Touch ID or your login password)\"",
                    "where_word=\"this PC (Windows Hello or your Windows password)\"",
                    "echo \"Approve on ${where_word}…\"",
                    "Approve on ${where_word}, or on your phone: ${_VPC_LINE}. Waiting for your phone… (5 minutes; the first answer wins)",
                    "Approve on your phone: ${_VPC_LINE}. Waiting for your phone… (5 minutes)",
                    "no_phone_confirm: the phone card could not be raised (above)",
                    "Nobody answered on this computer.",
                    "Confirmed on your phone.",
                    "(no phone card: %s)",
                    "card %s on %s'\"'\"'s Thread%s (%s)",
                    "(\" — \" + p[\"thread\"])",
                    "Confirming from this terminal (this server has no owner login yet)…",
                    "Which owner login on this server (k2 users lists them): ",
                    "Sign-in password for owner login %s: ",
                    "printf 'Vault password: '",
                    "isn't there (is the ",
                ]
                .iter(),
            )
        {
            // The shell spells the default brand's names.
            let l = l
                .replace(PRODUCT_NAME, "K2")
                .replace(&format!("{} ", k2_client::brand::CLI_NAME), "k2 ");
            assert!(src.contains(&l), "not in vault.sh: {l}");
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
