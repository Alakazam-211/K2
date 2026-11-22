//! The requests every `k5 app` verb makes, as the shell CLI makes them
//! (`cli/src/compute-lib.sh`): `_compute_send`, `_compute_split`,
//! `_compute_call`, `_compute_fail` with the App refusal registry, and
//! `_compute_json`.
//!
//! - [`Ctx::send`] is `raw="$(_compute_send …)"; _compute_split "$raw"`:
//!   the cell socket first when there is a passport with a live one, then
//!   TCP ([`Client::compute_send`]). What the shell holds is curl's output
//!   with its `-w` status line, one `\n000` for each connect `_k2_curl`
//!   tried again, and `\n000` more when curl failed over TCP (`|| printf`).
//!   So an answer split from that has the tries' `\n000` lines in front of
//!   its body, and a failure has a body of `\n000` lines, which
//!   `_compute_fail` prints as `daemon_error: …`. A POST that may have run
//!   on the cell socket, and a session whose TCP credential would be the
//!   owner token from disk, end the command with the transport's line and
//!   exit 1 (`_cli_send_unanswered`, `_k2_sock_fallback_token`).
//! - [`Ctx::call`] is `_compute_call`: any status but 200 is
//!   [`Ctx::fail`].
//! - [`Ctx::fail`] is `_compute_fail` with `_K2_REFUSAL_AREA=app`: the
//!   daemon's error as one JSON line on stderr, with the registry's `next`
//!   and `explain` when its code has a row, then `fix: <next>` unless a
//!   word of the command is `--json`; exit 2 for a 400 or 413, else 3.
//! - [`Cj`] is `_compute_json`: the body's keys in the order given, an
//!   empty value left out, printed as Python's `json.dumps` prints.
//!   [`Given`] is its whole rule on top of [`Cj`]: the pairs as the
//!   caller names them (`K2_CJ_<name>`), the secrets first.
//!
//! A 404 is 3: `_compute_fail` takes its exit for a 404 from the caller
//! and never reads the environment. Where this differs from the shell: a
//! body that is not UTF-8 makes the shell's Python fail on most locales,
//! so its line is the no-answer one; that is what this prints.

use crate::exit::Exit;
use crate::out;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::prelude::Closing;
use crate::transport::{permission, Channel, Client, Error, Response};
use k2_client::{cli_name, product_name};

/// `_compute_fail`'s line when its Python printed nothing; `STATUS` is the
/// status.
const NO_ANSWER: &str = concat!(
    "{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the ",
    product_name!(),
    " daemon (status STATUS)\"}"
);

/// `"k2 explain " + id`.
const EXPLAIN: &str = concat!(cli_name!(), " explain ");

/// The App refusal registry's rows, as `_k2_refusals_json` prints them
/// (`cli/src/study-app-api.sh`, generated from `crates/k2-core/src/refusals`).
const REGISTRY_SOURCE: &str = include_str!("../../../../../cli/src/study-app-api.sh");

/// The registry's JSON: the heredoc of `_k2_refusals_json`.
pub fn registry_json() -> &'static str {
    const HEAD: &str = "_k2_refusals_json() {\n    cat <<'EOF'\n";
    let body = REGISTRY_SOURCE
        .split_once(HEAD)
        .map_or("", |(_, rest)| rest);
    body.split_once("\nEOF").map_or("", |(json, _)| json)
}

/// What `_compute_split` made of `_compute_send`'s output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// `_CP_STATUS`: three digits, `000` for no answer.
    pub status: String,
    /// `_CP_BODY`.
    pub body: Vec<u8>,
}

impl Split {
    pub fn ok(&self) -> bool {
        self.status == "200"
    }
}

/// One `k5 app` run: its client, the EXIT trap's lines, and whether a
/// word of the command is `--json` (`_K2_ARGS_JSON`).
pub struct Ctx {
    pub client: Client,
    pub closing: Closing,
    pub args_json: bool,
    /// `<K2 home>/vault.sock`, where a confirm is asked
    /// ([`crate::confirm::confirm_token`]).
    pub vault_sock: std::path::PathBuf,
    /// `$PROJECT`: the checkout `_compute_ws` names outside a session.
    pub project: String,
    /// `_K2_REFUSAL_AREA=app` (`cmd_app` sets it): a refusal carries the
    /// registry's fix. Off where the shell reaches the same code from
    /// `skin` (`cmd_skin`), which leaves it empty.
    pub app_refusals: bool,
    /// `_K2_ASSIGN`: the command's `--assign` ([`super::assign`]), empty
    /// when it has none. Every POST to `/cli/app/…` carries it.
    pub assign: String,
}

impl Ctx {
    pub fn new(client: Client, args_json: bool) -> Ctx {
        Ctx {
            client,
            closing: Closing::default(),
            args_json,
            vault_sock: std::path::PathBuf::new(),
            project: String::new(),
            app_refusals: true,
            assign: String::new(),
        }
    }

    /// `_compute_ws <explicit>`: the explicit workspace, else this
    /// checkout outside a session, else nothing (a passport names its
    /// own).
    pub fn ws(&self, explicit: &str) -> String {
        if !explicit.is_empty() {
            explicit.to_string()
        } else if self.in_session() {
            String::new()
        } else {
            self.project.clone()
        }
    }

    /// `K2_CP_MAX_TIME=<secs> _app_rt_post <path> <body>`: the POST with
    /// curl's `--max-time` at `secs`.
    pub fn post_max(&mut self, path: &str, body: &str, secs: u64) -> Result<Split, Exit> {
        let before = self.client.compute_post_max_time;
        self.client.compute_post_max_time = Some(std::time::Duration::from_secs(secs));
        let s = self.send(true, path, body, false);
        self.client.compute_post_max_time = before;
        s
    }

    /// `_compute_send <GET|POST> <path> <payload> [tcp]`, then
    /// `_compute_split`. `Err` when the command ended (its line printed).
    /// The path is `_compute_assign_path`'s first ([`super::assign::path`]).
    pub fn send(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<Split, Exit> {
        let path = super::assign::path(post, path, &self.assign);
        let result = self.client.compute_send(post, &path, payload, tcp);
        let refused = self.client.refused_before_answer();
        let raw = match &result {
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                return Err(Exit::Error);
            }
            Err(Error::NoAnswer(n)) => {
                out::stderr_line(&n.to_string());
                return Err(Exit::Error);
            }
            other => held(other, refused),
        };
        let split = split(&raw);
        let channel = result.as_ref().map_or(Channel::Tcp, |r| r.channel);
        let noted = Response {
            status: 0,
            body: split.body.clone(),
            channel,
            permission: permission::note(&split.body),
            server_version: None,
        };
        let passport = self.client.passport().is_some();
        self.closing.answer(&noted, passport);
        Ok(split)
    }

    /// For [`crate::confirm::confirm_token`].
    pub fn confirm(&mut self) -> crate::confirm::Confirm<'_> {
        let passport = self.client.passport().is_some();
        crate::confirm::Confirm {
            client: &mut self.client,
            sock: &self.vault_sock,
            closing: &mut self.closing,
            passport,
        }
    }

    /// `_compute_in_session`: the call carries a session passport.
    pub fn in_session(&self) -> bool {
        matches!(
            self.client.auth,
            crate::transport::client::Auth::Passport { .. }
        )
    }

    /// `_compute_call`: [`Ctx::send`], and any status but 200 is
    /// [`Ctx::fail`]. The body when it is 200.
    pub fn call(&mut self, post: bool, path: &str, payload: &str) -> Result<Vec<u8>, Exit> {
        self.call_on(post, path, payload, false)
    }

    /// `_compute_call … tcp`.
    pub fn call_on(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<Vec<u8>, Exit> {
        let s = self.send(post, path, payload, tcp)?;
        if s.ok() {
            Ok(s.body)
        } else {
            Err(self.fail(&s))
        }
    }

    /// `_compute_fail "$_CP_STATUS" "$_CP_BODY"`: its lines on stderr, and
    /// the exit.
    pub fn fail(&self, s: &Split) -> Exit {
        let line = fail_line_in(&s.status, &s.body, self.args_json, self.app_refusals);
        out::stderr_line(&line);
        fail_exit(&s.status)
    }
}

/// What `$(_compute_send …)` holds: NULs gone, trailing newlines gone.
pub fn held(result: &Result<Response, Error>, refused: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    for _ in 0..refused {
        raw.extend_from_slice(b"\n000");
    }
    match result {
        Ok(r) => {
            raw.extend_from_slice(&r.body);
            raw.extend_from_slice(format!("\n{:03}", r.status).as_bytes());
        }
        // curl's own `-w` line for the try that failed, then the shell's
        // `|| printf '\n000'`.
        Err(_) => raw.extend_from_slice(b"\n000\n000"),
    }
    raw.retain(|b| *b != 0);
    while raw.last() == Some(&b'\n') {
        raw.pop();
    }
    raw
}

/// `_compute_split "$raw"`.
pub fn split(raw: &[u8]) -> Split {
    match raw.iter().rposition(|b| *b == b'\n') {
        Some(i) => {
            let status = String::from_utf8_lossy(&raw[i + 1..]).into_owned();
            Split {
                status: if status.is_empty() {
                    "000".to_string()
                } else {
                    status
                },
                body: raw[..i].to_vec(),
            }
        }
        None => {
            let status = String::from_utf8_lossy(raw).into_owned();
            Split {
                status: if status.is_empty() {
                    "000".to_string()
                } else {
                    status
                },
                body: Vec::new(),
            }
        }
    }
}

/// `_compute_fail`'s exit for a status: 2 for a bad request, 3 for the
/// rest.
pub fn fail_exit(status: &str) -> Exit {
    match status {
        "400" | "413" => Exit::Usage,
        _ => Exit::Refused,
    }
}

/// `_compute_fail`'s lines (without the last newline): what its Python
/// printed, `$(…)` taking the trailing newlines off, or the no-answer line
/// when it printed nothing.
pub fn fail_line(status: &str, body: &[u8], args_json: bool) -> String {
    fail_line_in(status, body, args_json, true)
}

/// [`fail_line`], with the registry's fix only when `app` (the shell's
/// `_K2_REFUSAL_AREA` is `app`).
pub fn fail_line_in(status: &str, body: &[u8], args_json: bool, app: bool) -> String {
    let mut line = if body.is_empty() {
        String::new()
    } else {
        fail_python(body, args_json, app)
    };
    while line.ends_with('\n') {
        line.pop();
    }
    if line.is_empty() {
        line = NO_ANSWER.replace("STATUS", if status.is_empty() { "none" } else { status });
    }
    line
}

/// `_compute_fail`'s Python on the body. Nothing when it raises (the shell
/// sends its stderr to /dev/null).
fn fail_python(body: &[u8], args_json: bool, app: bool) -> String {
    let Ok(raw) = std::str::from_utf8(body) else {
        return String::new();
    };
    // `_k2_hint_fill` first: a hint template takes this binary's names.
    let v = match json::parse(raw).map(crate::transport::permission::hints_filled) {
        Ok(v) => v,
        Err(_) => {
            let head: String = raw.chars().take(200).collect();
            return format!("daemon_error: {head}\n");
        }
    };
    let e = match &v {
        Value::Object(pairs) => super::super::get(pairs, "error").cloned(),
        _ => None,
    };
    let s = |t: &str| Value::Str(Text::from(t));
    let mut out: Vec<(Text, Value)> = match &e {
        Some(Value::Object(pairs)) => vec![
            (
                Text::from("error"),
                super::super::get(pairs, "code")
                    .cloned()
                    .unwrap_or_else(|| s("error")),
            ),
            (
                Text::from("hint"),
                super::super::get(pairs, "hint")
                    .cloned()
                    .unwrap_or_else(|| s("")),
            ),
        ],
        Some(Value::Str(code)) if message(&v).is_some() => vec![
            (Text::from("error"), Value::Str(code.clone())),
            (
                Text::from("hint"),
                message(&v).cloned().unwrap_or(Value::Null),
            ),
        ],
        other => vec![
            (Text::from("error"), s("error")),
            (
                Text::from("hint"),
                s(&py::str_of(other.as_ref().unwrap_or(&Value::Null))),
            ),
        ],
    };
    let code = out[0].1.clone();
    let row = if !app || code == s("error") {
        None
    } else {
        registry_row(&code)
    };
    let mut fix = None;
    if let Some((id, f)) = row {
        out.push((Text::from("next"), f.clone()));
        out.push((
            Text::from("explain"),
            s(&format!("{EXPLAIN}{}", py::str_of(&id))),
        ));
        fix = Some(f);
    }
    let Ok(mut text) = json::to_string(&Value::Object(out), &Options::default()) else {
        return String::new();
    };
    text.push('\n');
    if let Some(f) = fix {
        if !args_json {
            match f {
                Value::Str(_) => text.push_str(&format!("fix: {}\n", py::str_of(&f))),
                // `"fix: " + row["fix"]` raises: the JSON line is out.
                _ => return text,
            }
        }
    }
    text
}

/// `v.get("message")` when it is a string.
fn message(v: &Value) -> Option<&Value> {
    match v {
        Value::Object(pairs) => {
            super::super::get(pairs, "message").filter(|m| matches!(m, Value::Str(_)))
        }
        _ => None,
    }
}

/// The registry row whose `id` is `code`, else the first whose `aliases`
/// hold it: its `id` and `fix`.
fn registry_row(code: &Value) -> Option<(Value, Value)> {
    let Ok(Value::Object(reg)) = json::parse(registry_json()) else {
        return None;
    };
    let Some(Value::Array(rows)) = super::super::get(&reg, "rows") else {
        return None;
    };
    let field = |r: &Value, k: &str| match r {
        Value::Object(p) => super::super::get(p, k).cloned(),
        _ => None,
    };
    let by_id = rows.iter().find(|r| field(r, "id").as_ref() == Some(code));
    let by_alias = || {
        rows.iter().find(|r| match field(r, "aliases") {
            Some(Value::Array(a)) => a.contains(code),
            _ => false,
        })
    };
    let r = by_id.or_else(by_alias)?;
    Some((field(r, "id")?, field(r, "fix")?))
}

/// `_compute_json K2_CJ_<key>=<value> …`: the pairs in order, each kind
/// as its suffix says, an empty value left out.
#[derive(Debug, Default, Clone)]
pub struct Cj {
    pairs: Vec<(Text, Value)>,
}

impl Cj {
    pub fn new() -> Cj {
        Cj::default()
    }

    fn put(mut self, key: &str, v: Value) -> Cj {
        let key = Text::from(key);
        match self.pairs.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = v,
            None => self.pairs.push((key, v)),
        }
        self
    }

    /// `K2_CJ_<key>=<value>`: a string.
    pub fn s(self, key: &str, value: &str) -> Cj {
        if value.is_empty() {
            return self;
        }
        self.put(key, Value::Str(Text::from(value)))
    }

    /// `K2_CJ_<key>_b=<value>`: `true` for `1`, else `false`.
    pub fn b(self, key: &str, value: &str) -> Cj {
        if value.is_empty() {
            return self;
        }
        self.put(key, Value::Bool(value == "1"))
    }

    /// `K2_CJ_<key>_n=<value>`: `int(value)`, or `float(value)` when it
    /// has a `.`. Callers hand it only digits (their loops check), so a
    /// value Python's `int` would refuse never comes here.
    pub fn n(self, key: &str, value: &str) -> Cj {
        if value.is_empty() {
            return self;
        }
        let v = if value.contains('.') {
            value.parse().map_or(Value::Null, Value::Float)
        } else {
            let digits = value.trim_start_matches('0');
            Value::Int(if digits.is_empty() { "0" } else { digits }.to_string())
        };
        self.put(key, v)
    }

    /// `K2_CJ_<key>_j=<value>`: `json.loads(value)`. Callers hand it only
    /// JSON they made.
    pub fn j(self, key: &str, value: &str) -> Cj {
        if value.is_empty() {
            return self;
        }
        match json::parse(value) {
            Ok(v) => self.put(key, v),
            Err(_) => self,
        }
    }

    /// The body, as `print(json.dumps(out))` held in `$(…)`.
    pub fn body(&self) -> String {
        json::to_string(&Value::Object(self.pairs.clone()), &Options::default())
            .unwrap_or_else(|_| "{}".to_string())
    }
}

/// `_compute_json`'s pairs as the caller gives them: `K2_CJ_<name>=<v>`,
/// a name given again keeping its first place with the last value (the
/// Python's `given` dict). The body takes the secrets (`_s`, `…Token`)
/// first, then the rest, each in the order given, an empty value left
/// out; each value's kind is its name's suffix, as [`Cj`] writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Given {
    pairs: Vec<(String, Vec<u8>)>,
}

impl Given {
    pub fn new() -> Given {
        Given::default()
    }

    /// `K2_CJ_<name>=<v>`.
    pub fn put(self, name: &str, v: &str) -> Given {
        self.put_bytes(name, v.as_bytes())
    }

    /// `K2_CJ_<name>=<v>` for a value that may not be UTF-8 (a file's
    /// text read with `$(cat …)`): the Python reads each byte that isn't
    /// as `surrogateescape` does, and `json.dumps` writes it `\udcXX`.
    /// Only a string's value (`_s`, or no suffix) comes here that way.
    pub fn put_bytes(mut self, name: &str, v: &[u8]) -> Given {
        match self.pairs.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = v.to_vec(),
            None => self.pairs.push((name.to_string(), v.to_vec())),
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// The pairs of `other` after these.
    pub fn then(mut self, other: &Given) -> Given {
        for (k, v) in &other.pairs {
            self = self.put_bytes(k, v);
        }
        self
    }

    /// `print(json.dumps(out))`.
    pub fn body(&self) -> String {
        let mut cj = Cj::new();
        for secret in [true, false] {
            for (k, v) in &self.pairs {
                if (k.ends_with("_s") || k.ends_with("Token")) != secret || v.is_empty() {
                    continue;
                }
                let Ok(v) = std::str::from_utf8(v) else {
                    let key = k.strip_suffix("_s").unwrap_or(k);
                    cj = cj.put(key, crate::families::approve::cp::env_text(v));
                    continue;
                };
                cj = if let Some(key) = k.strip_suffix("_s") {
                    cj.s(key, v)
                } else if let Some(key) = k.strip_suffix("_n") {
                    cj.n(key, v)
                } else if let Some(key) = k.strip_suffix("_b") {
                    cj.b(key, v)
                } else if let Some(key) = k.strip_suffix("_j") {
                    cj.j(key, v)
                } else {
                    cj.s(k, v)
                };
            }
        }
        cj.body()
    }
}

/// Whether `--json` is one of the words after the program's name
/// (`_K2_ARGS_JSON`).
pub fn args_json(words: &[&str]) -> bool {
    words.contains(&"--json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pairs_as_compute_json_makes_them() {
        let g = Given::new()
            .put("app", "a")
            .put("memory", "8G")
            .put("confirmToken", "t")
            .put("memory", "");
        assert_eq!(g.body(), r#"{"confirmToken": "t", "app": "a"}"#);
        let g = Given::new()
            .put("io", "none")
            .put("io_j", r#"{"rbps": "1"}"#);
        assert_eq!(g.body(), r#"{"io": {"rbps": "1"}}"#);
    }

    #[test]
    fn a_value_that_is_not_utf8_as_surrogateescape_reads_it() {
        let g = Given::new()
            .put("app", "a")
            .put_bytes("brief", b"caf\xc3\xa9 \xff\xfe!");
        assert_eq!(
            g.body(),
            r#"{"app": "a", "brief": "caf\u00e9 \udcff\udcfe!"}"#
        );
    }
}
