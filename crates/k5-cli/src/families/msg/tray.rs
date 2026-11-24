//! `k5 msg <workspace> --inbox-silent|--inbox-wake …`: a tray package, as
//! the shell's `cmd_msg_inbox_form` (`cli/src/msg-inbox-staging.sh`)
//! sends it.
//!
//! What runs, after `cmd_msg_v2`'s checks ([`super::send::parse`]), in the
//! shell's order:
//!
//! 1. The target is the first word, whatever it is; the rest less the
//!    `--inbox*` flags are read by [`parse`]. Federation's wire form is
//!    three lines, exit 2. An unknown flag is one line, exit 2; a value
//!    flag as the last word is the shell's `_need_value` line, exit 2.
//! 2. The files: `--path`, else every word that is a file here, else the
//!    first word when it looks like a path. Words that are not delivered
//!    files join `--body` as the sender's brief.
//! 3. The send key ([`super::send_once::init`]).
//! 4. Files: each made absolute (Python's `os.path.abspath` of
//!    `expanduser`); one that is no file is two lines, exit 2.
//!    - On this machine (`BASE_URL` on loopback), the paths go to
//!      `/cli/inbox/deliver` (`path=` for one, `paths=` a JSON list for
//!      several), sent once by the key. An answer saying the daemon can't
//!      read the path turns to the upload below.
//!    - Else, or then: several files are packed into one flat tar.gz
//!      ([`pack`], Python's `tarfile` as the shell runs it), staged under
//!      the workspace's `.k2/tmp/msg-inbox/` on the daemon through
//!      `/cli/fs/upload-binary` (under 50 MB, base64 in one JSON body) or
//!      `/cli/fs/upload-chunk` (8 MiB parts), delivered from there
//!      (`/cli/inbox/deliver-bundle` or `deliver`), and the staged copy
//!      deleted once it landed (`/cli/fs/delete`, best effort).
//! 5. No files: `--title` is needed (two lines, exit 2). Then
//!    `/cli/inbox/compose`, sent once; for `--inbox-wake` on this server a
//!    second send to `/cli/workspace/msg` with the inbox pointer (the
//!    compose answer's id and title, or the typed title, each made one
//!    line by [`one_line`]), and the two answers merged into one JSON
//!    line. To another server
//!    (`agent::host`) the wake rides the compose.
//! 6. The answer's reader ([`map_response`]): exit 3 for a gate, 1 for an
//!    error, 0 when it landed (a wake that failed is a note).
//! 7. The EXIT trap's lines.
//!
//! The temp files (`mktemp`) and the upload body files are made with the
//! system's `mktemp`, as the shell makes them: its own error
//! line on a `TMPDIR` that can't be used.
//!
//! The shell answers itself (see [`parse`]): a file word starting with
//! `~` and a name (`~user/…`), whose home Python looks up.
//!
//! Where it differs: the upload and delete requests connect within 5 s,
//! where the shell's own curl gives them 10 s.

use super::super::{end_in_pipe, traceback, Shown};
use super::send_once::{self, Key};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Client, Error, OwnCurl};
use k2_client::{cli_name, product_name};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub mod pack;

/// The routes.
pub const DELIVER: &str = "/cli/inbox/deliver";
pub const DELIVER_BUNDLE: &str = "/cli/inbox/deliver-bundle";
pub const COMPOSE: &str = "/cli/inbox/compose";
pub const LIVE: &str = "/cli/workspace/msg";
pub const RESOLVE: &str = "/cli/workspace/resolve";
pub const UPLOAD_BINARY: &str = "/cli/fs/upload-binary";
pub const UPLOAD_CHUNK: &str = "/cli/fs/upload-chunk";
pub const DELETE: &str = "/cli/fs/delete";

/// `_MSG_INBOX_SINGLE_SHOT_MAX` and `_MSG_INBOX_CHUNK_BYTES`.
pub const SINGLE_SHOT_MAX: u64 = 50_000_000;
pub const CHUNK_BYTES: u64 = 8_388_608;

/// The tray form's words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub target: String,
    /// `silent` or `wake`.
    pub mode: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub from: String,
    pub path: String,
    pub summary: String,
    pub idem_key: String,
    pub positional: Vec<String>,
}

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Form(Box<Form>),
    /// Stderr and the exit, nothing sent.
    Stop(String, Exit),
    /// The shell answers it.
    Shell,
}

/// The usage line for a tray form.
fn usage(mode: &str) -> String {
    format!(
        concat!(
            "Usage: ",
            cli_name!(),
            " msg <workspace> --inbox-{} <path> [path…] [--title \"...\"] [--body \"...\"] [--from <name>]"
        ),
        mode
    )
}

/// The wire-form lines.
pub fn wire_form(target: &str) -> String {
    format!(
        concat!(
            cli_name!(),
            " msg: '{}' looks like federation wire form (<fp>::<workspace>::<agent>).\n",
            "  Tray file send uses the user form:  ",
            cli_name!(),
            " msg <agent>::<host> --inbox-wake <path>\n",
            "  Wire form is for:              ",
            cli_name!(),
            " fed send <fp>::<ws>::<agent> \"text\"\n"
        ),
        target
    )
}

/// `_msg_inbox_teach_fs_auth`.
pub fn teach_fs_auth() -> &'static str {
    concat!(
        "Error: staging a file onto the host requires an owner or ",
        product_name!(),
        " Connect user token.\n",
        "In-cell agent passports cannot call /cli/fs/* for remote upload.\n",
        "Same-machine: ensure the daemon can read the path (loopback) so no upload is needed.\n",
        "Remote: run ",
        cli_name!(),
        " as the human/owner session against that server, or use a connect-user token.\n"
    )
}

/// `cmd_msg_v2`'s split, then `_cmd_msg_inbox_form_inner`'s loop and
/// file pick. `words` are all of `msg`'s words; `wake` says which flag.
pub fn parse(words: &[&str], wake: bool) -> Parsed {
    let mode = if wake { "wake" } else { "silent" };
    let Some(first) = words.first() else {
        return Parsed::Shell;
    };
    let mut f = Form {
        target: (*first).to_string(),
        mode: mode.into(),
        ..Form::default()
    };
    let rest: Vec<&str> = words[1..]
        .iter()
        .copied()
        .filter(|w| !matches!(*w, "--inbox" | "--inbox-silent" | "--inbox-wake"))
        .collect();
    if f.target.matches("::").count() >= 2 {
        return Parsed::Stop(wire_form(&f.target), Exit::Usage);
    }
    let mut i = 0;
    while let Some(w) = rest.get(i) {
        let next = rest.get(i + 1).copied();
        let slot = match *w {
            "--idempotency-key" => match next {
                Some(k) if !k.is_empty() => {
                    f.idem_key = k.to_string();
                    i += 2;
                    continue;
                }
                _ => {
                    return Parsed::Stop(
                        "usage: --idempotency-key <key>  (the key a 'may have been delivered' line printed)\n".into(),
                        Exit::Usage,
                    )
                }
            },
            "--summary" => &mut f.summary,
            "--title" => &mut f.title,
            "--body" => &mut f.body,
            "--priority" => &mut f.priority,
            "--from" => &mut f.from,
            "--path" => &mut f.path,
            w if w.starts_with('-') => {
                return Parsed::Stop(
                    format!(
                        concat!("Error: unknown flag '{}' (try '", cli_name!(), " msg --help')\n"),
                        w
                    ),
                    Exit::Usage,
                )
            }
            w => {
                f.positional.push(w.to_string());
                i += 1;
                continue;
            }
        };
        let Some(v) = next else {
            return Parsed::Stop(super::send::needs_value(w), Exit::Usage);
        };
        *slot = v.to_string();
        i += 2;
    }
    if file_paths(&f).iter().any(|p| tilde_user(p)) {
        return Parsed::Shell;
    }
    Parsed::Form(Box::new(f))
}

/// `~name…`: Python looks the name's home up.
fn tilde_user(p: &str) -> bool {
    p.starts_with('~') && p.len() > 1 && !p.starts_with("~/")
}

/// The files the form delivers, as typed.
pub fn file_paths(f: &Form) -> Vec<String> {
    if !f.path.is_empty() {
        return vec![f.path.clone()];
    }
    let files: Vec<String> = f
        .positional
        .iter()
        .filter(|p| Path::new(p).is_file())
        .cloned()
        .collect();
    if !files.is_empty() {
        return files;
    }
    let Some(p0) = f.positional.first() else {
        return files;
    };
    // `case "$p0" in ./*|/*|~/*|*.md|…`: the `~/` pattern is tilde
    // expanded, so it is the home's folder.
    let home = std::env::var("HOME").unwrap_or_default();
    let path_like = p0.starts_with("./")
        || p0.starts_with('/')
        || (!home.is_empty() && p0.starts_with(&format!("{home}/")))
        || [
            ".md", ".txt", ".pdf", ".png", ".jpg", ".zip", ".json", ".csv",
        ]
        .iter()
        .any(|e| p0.ends_with(e));
    if path_like {
        vec![p0.clone()]
    } else {
        files
    }
}

/// `_msg_inbox_merge_brief`: `--body`, then the words that are not a
/// delivered file, joined by spaces.
pub fn merge_brief(existing: &str, files: &[String], positional: &[String]) -> String {
    let extras: Vec<&str> = positional
        .iter()
        .filter(|a| !files.contains(a))
        .map(String::as_str)
        .collect();
    let joined = extras.join(" ");
    match (existing.is_empty(), joined.is_empty()) {
        (false, false) => format!("{existing} {joined}"),
        (true, false) => joined,
        _ => existing.to_string(),
    }
}

/// Python's `os.path.abspath(os.path.expanduser(p))`, held by `$(…)`.
pub fn abspath(p: &str, home: Option<&str>, cwd: &Path) -> String {
    let expanded = match (p.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => {
            let h = h.trim_end_matches('/');
            format!("{}{rest}", if h.is_empty() { "/" } else { h })
        }
        _ => p.to_string(),
    };
    let joined = if expanded.starts_with('/') {
        expanded
    } else {
        format!("{}/{expanded}", cwd.display())
    };
    let mut held = normpath(&joined);
    while held.ends_with('\n') {
        held.pop();
    }
    held
}

/// Python's `posixpath.normpath` for an absolute path.
pub fn normpath(p: &str) -> String {
    let lead = if p.starts_with("//") && !p.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("{lead}{}", parts.join("/"))
}

/// Python's `json.dumps(v)`: `, ` and `: `, ASCII only.
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`.
fn dumps_compact(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            compact: true,
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .or_else(|_| {
        json::to_string(
            v,
            &Options {
                compact: true,
                ..Options::default()
            },
        )
    })
    .unwrap_or_default()
}

fn s(t: &str) -> Value {
    Value::Str(Text::from(t))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

fn get<'a>(pairs: &'a [(Text, Value)], key: &str) -> Option<&'a Value> {
    super::super::get(pairs, key)
}

/// `$TOKEN`, as the shell's own curls send it.
fn token(client: &Client) -> Option<String> {
    match &client.auth {
        Auth::None => None,
        Auth::Owner(t) => Some(t.clone()),
        Auth::Passport { passport, .. } => Some(passport.as_str().to_string()),
    }
}

/// `_msg_daemon_is_local`.
fn local(client: &Client) -> bool {
    !client.tls && matches!(client.host.as_str(), "127.0.0.1" | "localhost")
}

/// `k5 msg … --inbox-*`: the form's run, on the client the gates gave.
pub fn run(client: &mut Client, f: Form) -> Exit {
    let mut closing = Closing::default();
    let exit = match form(client, &mut closing, f) {
        Ok(e) | Err(e) => e,
    };
    closing.flush(client, exit);
    exit
}

/// The form's params, `<lead>` first.
fn params(f: &Form, lead: &str, wake_flag: &str, key: &Key) -> String {
    let mut p = format!(
        "{lead}&wake={}&mode={}&source=msg-inbox",
        quote(wake_flag),
        quote(&f.mode)
    );
    for (name, v) in [
        ("title", &f.title),
        ("body", &f.body),
        ("summary", &f.summary),
        ("from", &f.from),
    ] {
        if !v.is_empty() {
            p.push_str(&format!("&{name}={}", quote(v)));
        }
    }
    p.push_str(&format!("&idempotency_key={}", quote(&key.key)));
    p
}

/// `$(_k2_idem_send cli_post_form <ep> <params>)`, after
/// `_permission_note_reset`: the held answer, or the command's exit.
fn post(
    client: &mut Client,
    closing: &mut Closing,
    key: &Key,
    ep: &str,
    form: &str,
) -> Result<Vec<u8>, Exit> {
    closing.reset_note();
    let soft = send_once::send(client, closing, key, ep, Call::PostForm(&[form]))?;
    if soft.rc != 0 {
        return Err(Exit::from_curl(soft.rc));
    }
    Ok(soft.out)
}

fn form(client: &mut Client, closing: &mut Closing, mut f: Form) -> Result<Exit, Exit> {
    let files = file_paths(&f);
    if !files.is_empty() {
        f.body = merge_brief(&f.body, &files, &f.positional);
    }
    let wake_flag = if f.mode == "wake" { "true" } else { "false" };
    let key = send_once::init(client, &f.idem_key)?;
    if !files.is_empty() {
        return package(client, closing, &f, &files, wake_flag, &key);
    }
    if f.title.is_empty() {
        out::stderr_line(&usage(&f.mode));
        out::stderr_line(&format!(
            concat!(
                "   or: ",
                cli_name!(),
                " msg <workspace> --inbox-{} --title \"...\" [--body \"...\"]"
            ),
            f.mode
        ));
        return Err(Exit::Usage);
    }
    compose(client, closing, &f, wake_flag, &key)
}

fn package(
    client: &mut Client,
    closing: &mut Closing,
    f: &Form,
    files: &[String],
    wake_flag: &str,
    key: &Key,
) -> Result<Exit, Exit> {
    let home = std::env::var("HOME").ok();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut abs = Vec::with_capacity(files.len());
    for fp in files {
        let a = abspath(fp, home.as_deref(), &cwd);
        if !Path::new(&a).is_file() {
            out::stderr_line(&format!("Error: file not found: {fp}"));
            out::stderr_line(&usage(&f.mode));
            return Err(Exit::Usage);
        }
        abs.push(a);
    }
    let need_upload = !local(client);
    let target_q = quote(&f.target);
    if abs.len() > 1 {
        if !need_upload {
            let list = Value::Array(abs.iter().map(|p| s(p)).collect());
            let lead = format!("workspace={target_q}&paths={}", quote(&dumps(&list)));
            // `response=$(…) || true`.
            let response = post(
                client,
                closing,
                key,
                DELIVER,
                &params(f, &lead, wake_flag, key),
            )
            .unwrap_or_default();
            // An answer that the daemon can't read the paths: the upload.
            if !not_readable(&response) {
                return Ok(map_response(&response, wake_flag == "true"));
            }
        }
        let bundle = pack::bundle(&abs).ok_or(Exit::Error)?;
        let staged = match stage(client, &f.target, &bundle) {
            Ok(p) => p,
            Err(rc) => {
                let _ = std::fs::remove_file(&bundle);
                if rc == Exit::Refused {
                    return Err(Exit::Refused);
                }
                out::stderr_line(
                    "Error: failed to stage multi-file bundle on daemon for inbox deliver",
                );
                return Err(Exit::Error);
            }
        };
        let _ = std::fs::remove_file(&bundle);
        let lead = format!("workspace={target_q}&path={}", quote(&staged));
        let response = post(
            client,
            closing,
            key,
            DELIVER_BUNDLE,
            &params(f, &lead, wake_flag, key),
        )?;
        let rc = map_response(&response, wake_flag == "true");
        if rc == Exit::Ok {
            delete_staged(client, &staged);
        }
        return Ok(rc);
    }
    let single = |client: &mut Client, closing: &mut Closing, path: &str| {
        let lead = format!("workspace={target_q}&path={}", quote(path));
        post(
            client,
            closing,
            key,
            DELIVER,
            &params(f, &lead, wake_flag, key),
        )
    };
    if !need_upload {
        let response = single(client, closing, &abs[0])?;
        if !not_readable(&response) {
            return Ok(map_response(&response, wake_flag == "true"));
        }
    }
    let staged = match stage(client, &f.target, Path::new(&abs[0])) {
        Ok(p) => p,
        Err(Exit::Refused) => return Err(Exit::Refused),
        Err(_) => {
            out::stderr_line("Error: failed to stage local file on daemon for inbox deliver");
            return Err(Exit::Error);
        }
    };
    let response = single(client, closing, &staged)?;
    let rc = map_response(&response, wake_flag == "true");
    if rc == Exit::Ok {
        delete_staged(client, &staged);
    }
    Ok(rc)
}

fn compose(
    client: &mut Client,
    closing: &mut Closing,
    f: &Form,
    wake_flag: &str,
    key: &Key,
) -> Result<Exit, Exit> {
    let mut p = format!(
        "workspace={}&title={}&source=msg-inbox",
        quote(&f.target),
        quote(&f.title)
    );
    for (name, v) in [
        ("body", &f.body),
        ("summary", &f.summary),
        ("priority", &f.priority),
        ("from", &f.from),
    ] {
        if !v.is_empty() {
            p.push_str(&format!("&{name}={}", quote(v)));
        }
    }
    let idem = format!("idempotency_key={}", quote(&key.key));
    p.push('&');
    p.push_str(&idem);
    closing.reset_note();
    let (_, host) = super::super::read::split_remote(&f.target);
    if !host.is_empty() {
        p.push_str(&format!(
            "&wake={}&mode={}",
            quote(wake_flag),
            quote(&f.mode)
        ));
        let response = post_held(client, closing, key, COMPOSE, &p)?;
        return Ok(map_response(&response, wake_flag == "true"));
    }
    let response = post_held(client, closing, key, COMPOSE, &p)?;
    if wake_flag != "true" {
        return Ok(map_response(&response, false));
    }
    if let Some(n) = send_once::replay_note(&response) {
        out::stderr_line(&n);
    }
    let (id, title) = match mapped(&response, &f.title) {
        Mapped::Err(code, hint) => {
            let line = dumps_compact_ascii(&obj(vec![(
                "error",
                obj(vec![("code", s(&code)), ("hint", s(&hint))]),
            )]));
            out::stderr_line(&line);
            return Ok(
                if matches!(code.as_str(), "not_connected" | "gated" | "forbidden") {
                    Exit::Refused
                } else {
                    Exit::Error
                },
            );
        }
        Mapped::Ok(id, title) => (id, title),
        Mapped::ParseFail => return echo(&response).map(|()| Exit::Ok),
    };
    let from = if f.from.is_empty() {
        "external"
    } else {
        f.from.as_str()
    };
    let pointer = format!(
        concat!("[inbox:{}] {}\nOpen: ", cli_name!(), " inbox read {}"),
        id, title, id
    );
    let wake_params = format!(
        "workspace={}&text={}&from={}&wake=true&{idem}",
        quote(&f.target),
        quote(&pointer),
        quote(from)
    );
    let wake = post(client, closing, key, LIVE, &wake_params)?;
    match merge(&response, &wake) {
        Ok((line, note)) => {
            write_out(&format!("{line}\n"))?;
            if let Some(n) = note {
                out::stderr_line(&n);
            }
        }
        Err(tb) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
            echo(&response)?;
            out::stderr_line("note: package landed; wake notify failed (merge error)");
        }
    }
    Ok(Exit::Ok)
}

/// `post` with no `_permission_note_reset` of its own (compose resets
/// once, before it).
fn post_held(
    client: &mut Client,
    closing: &mut Closing,
    key: &Key,
    ep: &str,
    form: &str,
) -> Result<Vec<u8>, Exit> {
    let soft = send_once::send(client, closing, key, ep, Call::PostForm(&[form]))?;
    if soft.rc != 0 {
        return Err(Exit::from_curl(soft.rc));
    }
    Ok(soft.out)
}

fn write_out(text: &str) -> Result<(), Exit> {
    out::stdout(text).map_err(|e| out::after_stdout_error(&e))
}

/// `echo "$response"`.
fn echo(response: &[u8]) -> Result<(), Exit> {
    let mut b = response.to_vec();
    b.push(b'\n');
    out::write_flushed(&mut std::io::stdout().lock(), &b).map_err(|e| out::after_stdout_error(&e))
}

/// `json.dumps(v, separators=(",", ":"))`: compact, ASCII only.
fn dumps_compact_ascii(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            compact: true,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

/// What the compose's mapping Python prints, as the shell reads its
/// three lines back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mapped {
    ParseFail,
    /// The code (made one line) and the hint (as it came).
    Err(String, String),
    /// The id and the title, each made one line.
    Ok(String, String),
}

/// `str(v or "")`.
fn str_or_empty(v: Option<&Value>) -> String {
    v.filter(|v| py::truthy(v))
        .map(py::str_of)
        .unwrap_or_default()
}

/// The daemon's `inbox::one_line` as the shell's Python writes it:
/// trimmed (`str.strip()`), control, line-break, bidi and invisible
/// characters as `\u{..}`, cut at 2048 characters (then `...`), and a
/// third dash in a row as `\u{2d}`.
pub fn one_line(text: &str) -> String {
    const CAP: usize = 2048;
    let hidden = |c: char| {
        let u = u32::from(c);
        u < 0x20
            || (0x7f..=0x9f).contains(&u)
            || matches!(
                u,
                0x2028 | 0x2029 | 0x061c | 0x200e | 0x200f | 0x2060 | 0xfeff
            )
            || (0x202a..=0x202e).contains(&u)
            || (0x2066..=0x2069).contains(&u)
            || (0x200b..=0x200d).contains(&u)
            || (0xe0000..=0xe007f).contains(&u)
    };
    let text = text.trim_matches(super::super::py_space);
    let mut out = String::with_capacity(text.len());
    for c in text.chars().take(CAP) {
        if hidden(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    if text.chars().nth(CAP).is_some() {
        out.push_str("...");
    }
    let mut res = String::with_capacity(out.len());
    let mut dashes = 0;
    for c in out.chars() {
        if c != '-' {
            dashes = 0;
            res.push(c);
        } else if dashes == 2 {
            dashes = 0;
            res.push_str("\\u{2d}");
        } else {
            dashes += 1;
            res.push(c);
        }
    }
    res
}

/// The compose answer's mapping Python (`typed`: the `--title` given,
/// its `sys.argv[1]`), one field per line.
pub fn mapped(response: &[u8], typed: &str) -> Mapped {
    let Ok(d) = json::parse(&String::from_utf8_lossy(response)) else {
        return Mapped::ParseFail;
    };
    let Value::Object(pairs) = &d else {
        return Mapped::ParseFail;
    };
    match get(pairs, "error") {
        Some(Value::Object(e)) => {
            let code = str_or_empty(get(e, "code"));
            let hint = match get(e, "hint").filter(|v| py::truthy(v)) {
                Some(v) => py::str_of(v),
                None => code.clone(),
            };
            return Mapped::Err(one_line(&code), hint);
        }
        Some(v) if matches!(v, Value::Str(_)) && py::truthy(v) => {
            return Mapped::Err("error".into(), py::str_of(v));
        }
        _ => {}
    }
    if matches!(get(pairs, "ok"), Some(Value::Bool(false))) && get(pairs, "id").is_none() {
        return Mapped::Err("error".into(), "compose failed".into());
    }
    let title = match get(pairs, "title").filter(|v| py::truthy(v)) {
        Some(v) => py::str_of(v),
        None => typed.to_string(),
    };
    let id = str_or_empty(get(pairs, "id"));
    Mapped::Ok(one_line(&id), one_line(&title))
}

/// The merge Python on the two answers: the JSON line and the note, or
/// its traceback where it raises.
pub fn merge(pkg: &[u8], wake: &[u8]) -> Result<(String, Option<String>), String> {
    let pkg = json::parse(&String::from_utf8_lossy(pkg)).map_err(|_| {
        traceback(
            &[(3, "<module>")],
            "json.decoder.JSONDecodeError: Expecting value",
        )
    })?;
    let Value::Object(mut pkg) = pkg else {
        return Err(traceback(
            &[(8, "<module>")],
            &format!(
                "TypeError: '{}' object does not support item assignment",
                py::type_name(&pkg)
            ),
        ));
    };
    let wake = match json::parse(&String::from_utf8_lossy(wake)) {
        Ok(Value::Object(w)) => w,
        Ok(other) => {
            return Err(traceback(
                &[(9, "<module>")],
                &format!(
                    "AttributeError: '{}' object has no attribute 'get'",
                    py::type_name(&other)
                ),
            ))
        }
        Err(_) => {
            let raw = String::from_utf8_lossy(wake);
            let head: String = raw.chars().take(200).collect();
            vec![
                (Text::from("success"), Value::Bool(false)),
                (Text::from("reason"), s("wake_parse_error")),
                (Text::from("hint"), s(&head)),
            ]
        }
    };
    let g = |k: &str| get(&wake, k).cloned().unwrap_or(Value::Null);
    let or = |a: &str, b: &str| {
        let v = g(a);
        if py::truthy(&v) {
            v
        } else {
            g(b)
        }
    };
    let success = py::truthy(&g("success"));
    let w = obj(vec![
        ("success", Value::Bool(success)),
        ("reason", g("reason")),
        ("hint", g("hint")),
        (
            "targetSessionId",
            or("target_session_id", "targetSessionId"),
        ),
        ("woke", g("woke")),
        ("wakeMs", or("wake_ms", "wakeMs")),
        ("attempts", g("attempts")),
    ]);
    match pkg.iter_mut().find(|(k, _)| *k == Text::from("wake")) {
        Some(slot) => slot.1 = w,
        None => pkg.push((Text::from("wake"), w)),
    }
    let line = dumps_compact(&Value::Object(pkg));
    let note = (!success).then(|| {
        let reason = g("reason");
        let reason = if py::truthy(&reason) {
            py::str_of(&reason)
        } else {
            "wake_failed".into()
        };
        let hint = g("hint");
        let hint = if py::truthy(&hint) {
            format!(": {}", py::str_of(&hint))
        } else {
            String::new()
        };
        format!("note: package landed; wake notify failed ({reason}){hint}")
    });
    Ok((line, note))
}

/// `_msg_inbox_map_response <response> <wake>`: the replay note, the raw
/// answer when it is no error, then the reader's lines; its exit.
pub fn map_response(response: &[u8], wake: bool) -> Exit {
    if let Some(n) = send_once::replay_note(response) {
        out::stderr_line(&n);
    }
    let (is_error, shown) = reader(response, wake);
    if !is_error {
        if let Err(e) = echo(response) {
            return e;
        }
    }
    shown.print()
}

/// The two Pythons: whether the answer is an error (no echo then), and
/// what the second prints.
pub fn reader(response: &[u8], wake: bool) -> (bool, Shown) {
    let Ok(d) = json::parse(&String::from_utf8_lossy(response)) else {
        return (false, Shown::out(""));
    };
    let Value::Object(pairs) = &d else {
        return (
            false,
            Shown::err(
                traceback(
                    &[(7, "<module>")],
                    &format!(
                        "AttributeError: '{}' object has no attribute 'get'",
                        py::type_name(&d)
                    ),
                ),
                Exit::Error,
            ),
        );
    };
    let err = get(pairs, "error");
    let is_error = match err {
        Some(Value::Object(_)) => true,
        Some(Value::Str(t)) => py::truthy(&Value::Str(t.clone())),
        _ => false,
    };
    match err {
        Some(Value::Object(e)) => {
            let code = get(e, "code")
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or_else(|| s(""));
            let hint = get(e, "hint")
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or_else(|| code.clone());
            let refusal = matches!(&code, Value::Str(t)
                if matches!(t.as_str(), Some("not_connected" | "gated" | "forbidden")));
            let line = dumps_compact(&obj(vec![(
                "error",
                obj(vec![("code", code), ("hint", hint)]),
            )]));
            return (
                is_error,
                Shown::err(
                    format!("{line}\n"),
                    if refusal { Exit::Refused } else { Exit::Error },
                ),
            );
        }
        Some(Value::Str(t)) if is_error => {
            let line = dumps_compact(&obj(vec![(
                "error",
                obj(vec![("code", s("error")), ("hint", Value::Str(t.clone()))]),
            )]));
            return (is_error, Shown::err(format!("{line}\n"), Exit::Error));
        }
        _ => {}
    }
    if matches!(get(pairs, "ok"), Some(Value::Bool(false)))
        && get(pairs, "id").is_none()
        && get(pairs, "filename").is_none()
    {
        return (is_error, Shown::err("", Exit::Error));
    }
    let mut stderr = String::new();
    if let Some(Value::Str(t)) = get(pairs, "addressNote") {
        if py::truthy(&Value::Str(t.clone())) {
            stderr.push_str(&format!("note: {}\n", py::str_of(&Value::Str(t.clone()))));
        }
    }
    if wake {
        if let Some(Value::Object(w)) = get(pairs, "wake") {
            if matches!(get(w, "success"), Some(Value::Bool(false))) {
                let reason = get(w, "reason")
                    .filter(|v| py::truthy(v))
                    .map_or_else(|| "wake_failed".to_string(), py::str_of);
                let hint = get(w, "hint")
                    .filter(|v| py::truthy(v))
                    .map_or_else(String::new, |h| format!(": {}", py::str_of(h)));
                stderr.push_str(&format!(
                    "note: package landed; wake notify failed ({reason}){hint}\n"
                ));
            }
        }
    }
    (is_error, Shown::err(stderr, Exit::Ok))
}

/// Python's `str(x)` of a JSON value, lower-cased.
fn lower_str(v: &Value) -> String {
    py::str_of(v).to_lowercase()
}

/// `_msg_inbox_is_not_readable_error`.
pub fn not_readable(response: &[u8]) -> bool {
    let hit = |s: &str| s.contains("not a readable file") || s.contains("source path is not");
    let raw = String::from_utf8_lossy(response);
    let Ok(d) = json::parse(&raw) else {
        return hit(&raw.to_lowercase());
    };
    let Value::Object(pairs) = &d else {
        return false;
    };
    let s = match get(pairs, "error") {
        Some(Value::Object(e)) => ["hint", "message", "code", "error"]
            .iter()
            .map(|k| match get(e, k).filter(|v| py::truthy(v)) {
                Some(v) => py::str_of(v),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        Some(v) if py::truthy(v) => py::str_of(v),
        _ => String::new(),
    };
    hit(&s.to_lowercase())
}

/// `_msg_inbox_is_fs_auth_error`.
pub fn fs_auth_error(response: &[u8]) -> bool {
    const NEEDLES: [&str; 9] = [
        "invalid or missing token",
        "forbidden",
        "not owner",
        "owner_only",
        "unauthorized",
        "not authorized",
        "permission denied",
        "invalid token",
        "missing token",
    ];
    if response.is_empty() {
        return false;
    }
    let raw = String::from_utf8_lossy(response);
    let any = |s: &str| NEEDLES.iter().any(|n| s.contains(n));
    let Ok(d) = json::parse(&raw) else {
        let s = raw.to_lowercase();
        return any(&s) || s.contains("403");
    };
    let Value::Object(pairs) = &d else {
        return false;
    };
    let mut parts = Vec::new();
    match get(pairs, "error") {
        Some(Value::Object(e)) => {
            for k in ["code", "hint", "message", "error", "reason"] {
                match get(e, k) {
                    None | Some(Value::Null) => {}
                    Some(v) => parts.push(py::str_of(v)),
                }
            }
            let code = get(e, "code")
                .filter(|v| py::truthy(v))
                .map(lower_str)
                .unwrap_or_default();
            if matches!(
                code.as_str(),
                "forbidden"
                    | "owner_only"
                    | "unauthorized"
                    | "invalid_token"
                    | "not_owner"
                    | "gated"
                    | "auth"
                    | "permission_denied"
            ) {
                return true;
            }
        }
        Some(Value::Str(t)) => parts.push(py::str_of(&Value::Str(t.clone()))),
        None | Some(Value::Null) => {}
        Some(other) => parts.push(py::str_of(other)),
    }
    for k in ["code", "hint", "message", "status"] {
        match get(pairs, k) {
            None | Some(Value::Null) => {}
            Some(v) => parts.push(py::str_of(v)),
        }
    }
    let s = parts.join(" ").to_lowercase();
    any(&s) || s.contains("403")
}

/// `_msg_inbox_resolve_ws_path`: the workspace's folder on the daemon,
/// or empty.
fn resolve_ws_path(client: &mut Client, target: &str) -> String {
    let q = format!("q={}", quote(target));
    let body = match client.send(RESOLVE, Call::Request(&[q.as_str()])) {
        Ok(r) => send_once::held(&r.body),
        Err(Error::NoAnswer(_)) => end_in_pipe(),
        Err(_) => Vec::new(),
    };
    let Ok(Value::Object(pairs)) = json::parse(&String::from_utf8_lossy(&body)) else {
        return String::new();
    };
    match get(&pairs, "path").filter(|v| py::truthy(v)) {
        Some(v) => {
            let mut p = py::str_of(v);
            while p.ends_with('\n') {
                p.pop();
            }
            p
        }
        None => String::new(),
    }
}

/// `$(mktemp <template>)`: the system's, as the shell runs it (its error
/// line on stderr).
pub fn mktemp(template: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("mktemp")
        .arg(template)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut p = String::from_utf8_lossy(&out.stdout).into_owned();
    while p.ends_with('\n') {
        p.pop();
    }
    Some(PathBuf::from(p))
}

/// `${TMPDIR:-/tmp}`.
pub fn tmpdir() -> String {
    std::env::var("TMPDIR")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(crate::offline::default_tmpdir)
}

/// `_msg_inbox_stage_file <target> <local>`: the staged path, or
/// [`Exit::Refused`] (the teaching printed) or [`Exit::Error`].
fn stage(client: &mut Client, target: &str, local: &Path) -> Result<String, Exit> {
    let mut ws = resolve_ws_path(client, target);
    if ws.is_empty() {
        if target.starts_with('/') {
            ws = target.to_string();
        } else {
            out::stderr_line(&format!(
                "Error: cannot resolve workspace '{target}' on daemon for file staging"
            ));
            return Err(Exit::Error);
        }
    }
    let filename = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    upload(client, local, &format!("{ws}/.k2/tmp/msg-inbox"), &filename)
}

/// The shell's own curl for a JSON POST (`_msg_inbox_cli_post_json_file`
/// and `_msg_inbox_cli_post_json`): the held answer, empty on a failure.
fn own_post(client: &mut Client, ep: &str, body: &[u8], max: Duration) -> Vec<u8> {
    let tok = token(client);
    let how = OwnCurl {
        post: true,
        credential: tok.as_deref(),
        max_time: max,
        note: false,
    };
    match client.own_curl(ep, how, body) {
        Ok(r) => send_once::held(&r.body),
        Err(_) => Vec::new(),
    }
}

/// The upload answer's path, as its Python reads it.
fn uploaded_path(resp: &[u8]) -> Option<String> {
    let Ok(Value::Object(pairs)) = json::parse(&String::from_utf8_lossy(resp)) else {
        return None;
    };
    if get(&pairs, "error").is_some_and(py::truthy) {
        return None;
    }
    let p = get(&pairs, "path").filter(|v| py::truthy(v))?;
    let mut p = py::str_of(p);
    while p.ends_with('\n') {
        p.pop();
    }
    Some(p)
}

/// `_msg_inbox_upload_file <local> <dir> <filename>`.
fn upload(client: &mut Client, local: &Path, dir: &str, filename: &str) -> Result<String, Exit> {
    let data = std::fs::read(local).unwrap_or_default();
    let size = data.len() as u64;
    let body_file =
        mktemp(&format!("{}/k2-msg-inbox-upload.XXXXXX", tmpdir())).ok_or_else(|| {
            out::stderr_line("Error: cannot create temp file for upload body");
            Exit::Error
        })?;
    let fail_auth = |resp: &[u8]| -> Option<Exit> {
        fs_auth_error(resp).then(|| {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), teach_fs_auth().as_bytes());
            Exit::Refused
        })
    };
    if size < SINGLE_SHOT_MAX {
        let body = dumps_compact(&obj(vec![
            ("dir", s(dir)),
            ("filename", s(filename)),
            ("base64", s(&pack::base64(&data))),
        ]));
        let _ = std::fs::write(&body_file, body.as_bytes());
        let mt = if size > 0 {
            (60 + size / 500_000).min(600)
        } else {
            60
        };
        let resp = own_post(
            client,
            UPLOAD_BINARY,
            body.as_bytes(),
            Duration::from_secs(mt),
        );
        let _ = std::fs::remove_file(&body_file);
        if let Some(e) = fail_auth(&resp) {
            return Err(e);
        }
        return uploaded_path(&resp).ok_or_else(|| {
            if let Some(e) = fail_auth(&resp) {
                return e;
            }
            let shown = if resp.is_empty() {
                "no response".to_string()
            } else {
                String::from_utf8_lossy(&resp).into_owned()
            };
            out::stderr_line(&format!(
                "Error: upload-binary failed for {}: {shown}",
                local
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            ));
            Exit::Error
        });
    }
    let upload_id = format!(
        "msg-inbox-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        random15()
    );
    let mut offset = 0u64;
    let mut remote = String::new();
    while offset < size {
        let len = (size - offset).min(CHUNK_BYTES);
        let is_last = offset + len >= size;
        let chunk = &data[offset as usize..(offset + len) as usize];
        let mut pairs = vec![
            ("upload_id", s(&upload_id)),
            ("dir", s(dir)),
            ("filename", s(filename)),
            ("offset", Value::Int(offset.to_string())),
            ("base64", s(&pack::base64(chunk))),
            ("is_last", Value::Bool(is_last)),
        ];
        if offset == 0 {
            pairs.push(("total_bytes", Value::Int(size.to_string())));
        }
        let body = dumps_compact(&obj(pairs));
        let _ = std::fs::write(&body_file, body.as_bytes());
        let resp = own_post(
            client,
            UPLOAD_CHUNK,
            body.as_bytes(),
            Duration::from_secs(120),
        );
        if let Some(e) = fail_auth(&resp) {
            let _ = std::fs::remove_file(&body_file);
            return Err(e);
        }
        let shown = || {
            if resp.is_empty() {
                "no response".to_string()
            } else {
                String::from_utf8_lossy(&resp).into_owned()
            }
        };
        if is_last {
            match uploaded_path(&resp) {
                Some(p) => remote = p,
                None => {
                    let _ = std::fs::remove_file(&body_file);
                    if let Some(e) = fail_auth(&resp) {
                        return Err(e);
                    }
                    out::stderr_line(&format!("Error: upload-chunk finalize failed: {}", shown()));
                    return Err(Exit::Error);
                }
            }
        } else {
            let ok = matches!(json::parse(&String::from_utf8_lossy(&resp)),
                Ok(Value::Object(p)) if !get(&p, "error").is_some_and(py::truthy));
            if !ok {
                let _ = std::fs::remove_file(&body_file);
                if let Some(e) = fail_auth(&resp) {
                    return Err(e);
                }
                out::stderr_line(&format!(
                    "Error: upload-chunk failed at offset {offset}: {}",
                    shown()
                ));
                return Err(Exit::Error);
            }
        }
        offset += len;
    }
    let _ = std::fs::remove_file(&body_file);
    if remote.is_empty() {
        out::stderr_line("Error: chunked upload sent no data (empty file?)");
        return Err(Exit::Error);
    }
    Ok(remote)
}

/// bash's `$RANDOM`: 0 to 32767.
fn random15() -> u32 {
    let mut b = [0u8; 2];
    let _ = crate::sysrand::fill(&mut b);
    u32::from(u16::from_le_bytes(b) & 0x7fff)
}

/// `_msg_inbox_delete_staged`: best effort, nothing printed.
fn delete_staged(client: &mut Client, staged: &str) {
    if staged.is_empty() {
        return;
    }
    let body = dumps(&obj(vec![
        ("paths", Value::Array(vec![s(staged)])),
        ("permanent", Value::Bool(true)),
    ]));
    let _ = own_post(client, DELETE, body.as_bytes(), Duration::from_secs(30));
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

    /// The default build's names, for a compare with the shell's source.
    fn shell(text: &str) -> String {
        text.replace(&format!("{CLI_NAME} "), "k2 ")
            .replace(&format!("{PRODUCT_NAME} Connect"), "K2 Connect")
    }

    #[test]
    fn the_lines_are_the_shells() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/msg-inbox-staging.sh");
        let src =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for line in shell(teach_fs_auth()).lines() {
            assert!(
                src.contains(&format!("\n{line}\n")),
                "not in the shell: {line}"
            );
        }
        for line in shell(&wire_form("${target}")).lines() {
            let echoed = line.replace('"', "\\\"");
            assert!(src.contains(&echoed), "not in the shell: {echoed}");
        }
        let u = shell(&usage("${mode}")).replace('"', "\\\"");
        assert!(src.contains(&u), "not in the shell: {u}");
    }

    #[test]
    fn paths_are_made_absolute_as_python_does() {
        let cwd = Path::new("/w/proj");
        assert_eq!(abspath("a/../b/./c.md", Some("/h"), cwd), "/w/proj/b/c.md");
        assert_eq!(abspath("~/x", Some("/h/"), cwd), "/h/x");
        assert_eq!(abspath("~", Some("/h"), cwd), "/h");
        assert_eq!(abspath("//srv//x/..", None, cwd), "//srv");
        assert_eq!(abspath("/../..", None, cwd), "/");
        assert_eq!(
            merge_brief("B", &["a".into()], &["a".into(), "x".into(), "y".into()]),
            "B x y"
        );
        assert_eq!(merge_brief("", &["a".into()], &["a".into()]), "");
    }

    #[test]
    fn the_readers_do_what_the_pythons_do() {
        assert!(not_readable(
            br#"{"error":{"hint":"Source path is NOT here"}}"#
        ));
        assert!(not_readable(b"not a readable file, sorry"));
        assert!(!not_readable(br#"[1]"#));
        assert!(fs_auth_error(br#"{"error":{"code":"gated"}}"#));
        assert!(fs_auth_error(br#"{"status":403}"#));
        assert!(fs_auth_error(b"HTTP 403"));
        assert!(!fs_auth_error(b""));
        assert!(!fs_auth_error(
            br#"{"error":{"code":"bad_request","hint":"no"}}"#
        ));
        assert_eq!(
            mapped(br#"{"ok":true,"id":"c1","title":"a\tb\nc"}"#, "T"),
            Mapped::Ok("c1".into(), "a\\u{9}b\\u{a}c".into())
        );
        assert_eq!(
            mapped(br#"{"ok":true,"id":5,"title":""}"#, " x\u{1b}y "),
            Mapped::Ok("5".into(), "x\\u{1b}y".into())
        );
        assert_eq!(
            mapped(br#"{"error":{"code":"gated"}}"#, "T"),
            Mapped::Err("gated".into(), "gated".into())
        );
        assert_eq!(
            mapped(br#"{"error":{"code":"x\ny","hint":"a\nb"}}"#, "T"),
            Mapped::Err("x\\u{a}y".into(), "a\nb".into())
        );
        assert_eq!(
            mapped(br#"{"ok":false}"#, "T"),
            Mapped::Err("error".into(), "compose failed".into())
        );
        assert_eq!(mapped(b"nope", "T"), Mapped::ParseFail);
        assert_eq!(mapped(b"[1]", "T"), Mapped::ParseFail);
        assert_eq!(one_line("a----b-c---"), "a--\\u{2d}-b-c--\\u{2d}");
        assert_eq!(
            one_line("\u{1c} \u{202e}a\u{e0041}\u{3000}"),
            "\\u{202e}a\\u{e0041}"
        );
        let long = "x".repeat(2049);
        assert_eq!(one_line(&long), format!("{}...", "x".repeat(2048)));
        assert_eq!(one_line(&long[..2048]), "x".repeat(2048));
        let (line, note) = merge(br#"{"wake":0,"id":"c"}"#, b"oops").unwrap();
        assert_eq!(
            line,
            r#"{"wake":{"success":false,"reason":"wake_parse_error","hint":"oops","targetSessionId":null,"woke":null,"wakeMs":null,"attempts":null},"id":"c"}"#
        );
        assert_eq!(
            note.as_deref(),
            Some("note: package landed; wake notify failed (wake_parse_error): oops")
        );
        assert!(merge(br#"{"id":"c"}"#, b"[1]").is_err());
        let (err, shown) = reader(br#"{"error":"boom"}"#, false);
        assert!(err);
        assert_eq!(
            shown.stderr,
            "{\"error\":{\"code\":\"error\",\"hint\":\"boom\"}}\n"
        );
        let (err, shown) = reader(br#"{"ok":true,"wake":{"success":false}}"#, true);
        assert!(!err);
        assert_eq!(
            shown.stderr,
            "note: package landed; wake notify failed (wake_failed)\n"
        );
    }
}
