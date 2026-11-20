//! `k5 doc …` (also `docs`): live docs, as the shell CLI's `cmd_doc`
//! answers it (`cli/src/doc.sh`). Every verb is one daemon call (or a
//! few) on `/cli/docs/*`, then the shell's Python reader's lines.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates. `doc` alone, `doc ""`, `doc help` and a
//!    line with `-h` or `--help` anywhere skip the connection gate; the
//!    virtual desktop's guard stops all but a line with `-h` or `--help`.
//! 2. `docs` is the locked `id` tool: with `HOOK_SOCK` set, the
//!    stale-passport or require-passport lines, exit 3.
//! 3. `cores` first (its own help page, [`more::cores`]); then `-h` or
//!    `--help` anywhere, or no verb: the help page, and nothing is sent
//!    (help never acts).
//! 4. The verb's words, the common flags (`_doc_parse`), the request
//!    (`cli_send_json`, or `_doc_xfer` for whole files and tool calls:
//!    the cell socket with the passport, else loopback with the token; a
//!    POST again over TCP only after a failed connect or "not served"; a
//!    healed passport once on the auth gate's refusal, [`Ctx::xfer_raw`]),
//!    any status but 200 the refusal reader ([`fail`]), then the reader's
//!    lines ([`print`], [`tool`], [`more`]).
//!
//! An answer that is not UTF-8 is read as the shell's Python reads its
//! stdin, by its locale, and its bytes go back out as Python writes them
//! ([`py::stdin_text`]): never as U+FFFD.
//!
//! Handed to the shell before anything is printed or sent: a word that is
//! not UTF-8, `HOOK_SOCK` with a `CODEX_` variable, a port curl won't
//! take, a `python3` the binary can't tell (nor how it decodes its stdin:
//! [`py::decode_here`]), `--host`, `--token` or
//! `--token-stdin` anywhere (another server: the remote transport isn't
//! the binary's yet).
//!
//! A board verb on a `.k5board` file is the board family's answer, under
//! doc's gate, as the shell runs `cmd_board` there ([`board`]).

pub mod board;
pub mod more;
pub mod print;
pub mod py;
pub mod tool;

#[cfg(test)]
mod tests;

use self::py::{dumps, obj, s, Pr, R};
use super::sim::{die, Die};
use super::{curl_port, get, py_space, Raise};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::pylib::path as pypath;
use crate::out::{self, py as opy};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::{Auth, Client, Error};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["doc", "docs"];
/// The binary answers it; [`run`] hands the edge cases back.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "docs";

/// `_doc_xfer`'s `--max-time`.
const XFER_MAX_TIME: Duration = Duration::from_secs(300);

/// Whether the binary hands the line to the shell (see the module's
/// notes).
pub fn handed_back(words: &[&str]) -> bool {
    words
        .iter()
        .any(|w| matches!(*w, "--host" | "--token" | "--token-stdin") || w.starts_with("--host="))
}

fn wants_help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `k5 doc ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(n, _)| n)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if handed_back(&words)
        || !python::family_native(python::here())
        || py::decode_here().is_none()
    {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let skip = matches!(words.first(), None | Some(&("" | "help"))) || wants_help(&words);
    if !skip
        && prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, &words, skip, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    skip: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    let offline = wants_help(words);
    let no_token = settings.auth == Auth::None;
    if settings.no_owner_fallback && no_token && !offline {
        return Gate::VirtualDesktop.stop();
    }
    if !skip && (settings.port_text.as_deref().is_none_or(str::is_empty) || no_token) {
        return Gate::CannotConnect.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = settings.client(would_refuse).ok();
    let passport = matches!(settings.auth, Auth::Passport { .. });
    let in_cell = hook_sock.is_some() && passport && client.as_ref().is_some_and(|c| !c.force_tcp);
    let mut ctx = Ctx {
        client,
        closing: Closing::default(),
        json: false,
        ws: String::new(),
        ob: String::new(),
        fence: None,
        text: None,
        file: String::new(),
        in_cell,
        passport,
        tmp_made: false,
    };
    let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    let exit = match ctx.dispatch(&words) {
        Ok(e) | Err(e) => e,
    };
    if let Some(mut client) = ctx.client.take() {
        ctx.closing.flush(&mut client, exit);
    }
    exit
}

/// A step's end: the exit, everything already printed.
pub type Step = Result<Exit, Exit>;

/// What a line's calls share: the client, the EXIT trap's lines and the
/// common flags (`_DOC_*`).
pub struct Ctx {
    pub client: Option<Client>,
    pub closing: Closing,
    /// `_DOC_JSON`.
    pub json: bool,
    /// `_DOC_WS`.
    pub ws: String,
    /// `_DOC_OB`.
    pub ob: String,
    /// `_DOC_FENCE`: `--fence` / `--no-fence`, else the cell's default.
    pub fence: Option<bool>,
    /// `_DOC_TEXT` when `_DOC_TEXT_SET`.
    pub text: Option<String>,
    /// `_DOC_FILE`.
    pub file: String,
    /// `_doc_in_cell`.
    pub in_cell: bool,
    /// `SCOPED_TOKEN` is set.
    pub passport: bool,
    /// `_doc_tmp` ran.
    pub tmp_made: bool,
}

/// `printf '%s'` of a body the shell holds in a variable: no NULs.
fn held(body: &[u8]) -> Vec<u8> {
    super::without_nuls(body)
}

/// `$(cat file)`: no NULs, no trailing newlines.
pub fn substituted(body: &[u8]) -> Vec<u8> {
    let mut b = held(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// Python's `urllib.parse.urlencode` of pairs (`quote_plus`).
pub fn urlencode(pairs: &[(String, String)]) -> String {
    fn q(s: &str) -> String {
        let mut o = String::new();
        for &b in s.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                    o.push(b as char)
                }
                b' ' => o.push('+'),
                _ => o.push_str(&format!("%{b:02X}")),
            }
        }
        o
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", q(k), q(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A value of `_doc_body`'s pairs.
#[derive(Debug, Clone)]
pub enum B {
    /// A word, read by `_DOC_BODY_PY`'s rules (`@text`, `@b64`, `@true`,
    /// `@false`, `@int:N`).
    W(String),
    /// `argv`: a JSON list.
    Argv(Vec<String>),
}

pub fn w(t: impl Into<String>) -> B {
    B::W(t.into())
}

impl Ctx {
    pub fn usage(&self, hint: impl Into<String>) -> Exit {
        self.die("usage", hint, Exit::Usage)
    }

    /// `_doc_die CODE HINT [RC]`.
    pub fn die(&self, code: &'static str, hint: impl Into<String>, rc: Exit) -> Exit {
        Die {
            exit: rc,
            json: self.json,
            ..die(code, hint)
        }
        .print()
    }

    pub fn client(&mut self) -> &mut Client {
        self.client
            .as_mut()
            .expect("the connection gate let only a line with a port by")
    }

    /// `cli_send_json METHOD ROUTE PAYLOAD`, split: the status and the
    /// body as the shell holds it. A POST that went out with no answer
    /// ends the command.
    pub fn send(
        &mut self,
        post: bool,
        route: &str,
        payload: &str,
    ) -> Result<(String, Vec<u8>), Exit> {
        let passport = self.client().passport().is_some();
        match self.client().send_json(post, route, payload) {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok((format!("{:03}", r.status), held(&r.body)))
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(("000".into(), Vec::new())),
        }
    }

    /// `_doc_call`: the body of a 200, else the refusal's lines and exit.
    pub fn call(&mut self, post: bool, route: &str, payload: &str) -> Result<Vec<u8>, Exit> {
        let (status, body) = self.send(post, route, payload)?;
        if status != "200" {
            return Err(fail(&status, &body, self.json));
        }
        Ok(body)
    }

    /// `_doc_xfer`: the same channels at most 300 s each, the answer's
    /// bytes as they came; the permission note only on a refusal.
    ///
    /// The shell's `_doc_xfer` never tries a healed passport. The binary
    /// tries one once, and only on the daemon's auth gate refusal
    /// ([`crate::transport::client::gate_refusal`]): every doc route
    /// (`tool`, `write`, `export`, `show`, `export/fetch`) is refused
    /// there before its handler runs, so nothing was done and the call
    /// goes again. An answer the handler gave that holds the refusal's
    /// words (a document's text, a conflict) is never sent again, as the
    /// shell's. That is `scripts/cli-parity.sh`'s named difference
    /// `passport-heal-once`.
    pub fn xfer_raw(
        &mut self,
        post: bool,
        route: &str,
        payload: &str,
    ) -> Result<(String, Vec<u8>), Exit> {
        let passport = self.client().passport().is_some();
        let before = self.client().send_max_time;
        self.client().send_max_time = Some(XFER_MAX_TIME);
        self.client().heal_on_gate_only = true;
        let r = self.client().send_json(post, route, payload);
        self.client().heal_on_gate_only = false;
        self.client().send_max_time = before;
        match r {
            Ok(r) => {
                if r.status != 200 {
                    self.closing.answer(&r, passport);
                }
                Ok((format!("{:03}", r.status), r.body))
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(("000".into(), Vec::new())),
        }
    }

    /// `_doc_call_file`: the answer's bytes on a 200.
    pub fn xfer(&mut self, post: bool, route: &str, payload: &str) -> Result<Vec<u8>, Exit> {
        let (status, body) = self.xfer_raw(post, route, payload)?;
        if status != "200" {
            return Err(fail(&status, &substituted(&body), self.json));
        }
        Ok(body)
    }

    /// `_doc_path P`.
    pub fn path(&self, p: &str) -> String {
        let tcp = self.client.as_ref().is_some_and(|c| c.force_tcp);
        if self.ws.is_empty() && !tcp && !p.is_empty() && Path::new(p).exists() {
            pypath::abspath(p)
        } else {
            p.to_string()
        }
    }

    /// `[ $# -ge 2 ] || _doc_die usage HINT`: the word after `i`.
    pub fn need(&self, words: &[String], i: usize, hint: &str) -> Result<String, Exit> {
        words
            .get(i + 1)
            .cloned()
            .ok_or_else(|| self.usage(hint.to_string()))
    }

    /// `_doc_tmp`: `mktemp -d "${TMPDIR:-/tmp}/k2doc.XXXXXX"` once (the
    /// shell keeps whole-file bodies and answers there); where it can't be
    /// made, mktemp's line and `_doc_die tmp`, nothing sent. The binary
    /// keeps them in memory, so the folder goes again at once. Its line
    /// is in GNU mktemp's words wherever it runs; macOS's mktemp words it
    /// its own way (`scripts/cli-parity.sh`'s named difference
    /// `doc-mktemp-wording`).
    pub fn tmp(&mut self) -> Result<(), Exit> {
        if self.tmp_made {
            return Ok(());
        }
        let base = std::env::var("TMPDIR")
            .ok()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "/tmp".to_string());
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
            ^ u128::from(std::process::id());
        const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let six: String = (0..6)
            .map(|k| ALNUM[((n >> (k * 6)) % 62) as usize] as char)
            .collect();
        let dir = format!("{base}/k2doc.{six}");
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                let _ = std::fs::remove_dir(&dir);
                self.tmp_made = true;
                Ok(())
            }
            Err(e) => {
                out::stderr_line(&format!(
                    "mktemp: failed to create directory via template '{base}/k2doc.XXXXXX': {}",
                    tool::strerror(&e)
                ));
                Err(self.die("tmp", "can't make a temp folder", Exit::Usage))
            }
        }
    }

    /// `_doc_parse`: the common flags into `self`, the rest returned.
    pub fn parse(&mut self, words: &[String]) -> Result<Vec<String>, Exit> {
        let mut rest = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let a = words[i].as_str();
            match a {
                "--json" => self.json = true,
                "-w" | "--workspace" => {
                    self.ws = self.need(words, i, &format!("{a} needs a workspace"))?;
                    i += 1;
                }
                "--on-behalf-of" => {
                    self.ob = self.need(words, i, "--on-behalf-of needs a person")?;
                    i += 1;
                }
                "--fence" => self.fence = Some(true),
                "--no-fence" => self.fence = Some(false),
                "--text" => {
                    self.text = Some(self.need(words, i, "--text needs a value")?);
                    i += 1;
                }
                "--" => {
                    rest.extend(words[i + 1..].iter().cloned());
                    break;
                }
                _ => {
                    if let Some(v) = a.strip_prefix("--workspace=") {
                        self.ws = v.to_string();
                    } else if let Some(v) = a.strip_prefix("--on-behalf-of=") {
                        self.ob = v.to_string();
                    } else if let Some(v) = a.strip_prefix("--text=") {
                        self.text = Some(v.to_string());
                    } else {
                        rest.push(a.to_string());
                    }
                }
            }
            i += 1;
        }
        Ok(rest)
    }

    /// `_doc_one_path VERB`.
    pub fn one_path(&self, verb: &str, args: &[String]) -> Result<String, Exit> {
        if args.len() != 1 {
            return Err(self.usage(format!(
                "{CLI_NAME} doc {verb} takes one doc: {CLI_NAME} doc {verb} [flags] <file.md>"
            )));
        }
        Ok(self.path(&args[0]))
    }

    /// The bytes `@b64` and `@text` read: `--text`, else `--file`, else
    /// stdin.
    pub fn body_bytes(&self) -> R<Vec<u8>> {
        if let Some(t) = &self.text {
            return Ok(t.as_bytes().to_vec());
        }
        if !self.file.is_empty() {
            return std::fs::read(&self.file).map_err(|e| {
                Raise::at(
                    16,
                    crate::out::pylib::json::os_error(&e, &self.file).last_line(),
                )
            });
        }
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut b);
        Ok(b)
    }

    /// `_doc_body PATH [key value]...`: the JSON body, or what its Python
    /// raised (then the shell sends an empty body: the `$(...)` failed
    /// inside a word).
    pub fn body(&self, path: &str, kv: &[(&str, B)]) -> R<String> {
        let mut d: Vec<(Text, Value)> = Vec::new();
        let mut set = |k: &str, v: Value| {
            if let Some(slot) = d.iter_mut().find(|(t, _)| t.as_str() == Some(k)) {
                slot.1 = v;
            } else {
                d.push((Text::from(k), v));
            }
        };
        if !self.ws.is_empty() {
            set("workspace", s(&self.ws));
        }
        if !path.is_empty() {
            set("path", s(path));
        }
        if !self.ob.is_empty() {
            set("onBehalfOf", s(&self.ob));
        }
        for (k, v) in kv {
            let v = match v {
                B::Argv(list) => Value::Array(list.iter().map(|x| s(x)).collect()),
                B::W(v) if v == "@text" => {
                    let b = self.body_bytes()?;
                    match String::from_utf8(b) {
                        Ok(t) => s(&t),
                        Err(e) => return Err(utf8_raise(19, e.as_bytes(), e.utf8_error())),
                    }
                }
                B::W(v) if v == "@b64" => s(&super::store::widget::b64encode(&self.body_bytes()?)),
                B::W(v) if v == "@true" || v == "@false" => Value::Bool(v == "@true"),
                B::W(v) if v.starts_with("@int:") => match py::int_text(&v[5..]) {
                    Some(n) => Value::Int(n),
                    None => {
                        return Err(Raise::at(
                            31,
                            format!(
                                "ValueError: invalid literal for int() with base 10: {}",
                                opy::repr(&s(&v[5..]))
                            ),
                        ))
                    }
                },
                B::W(v) => s(v),
            };
            set(k, v);
        }
        Ok(dumps(&Value::Object(d)))
    }

    /// `_doc_body`, as a call's argument `"$(_doc_body ...)"`: a raise is
    /// its traceback on stderr, and the call goes with an empty body.
    pub fn body_arg(&self, path: &str, kv: &[(&str, B)]) -> String {
        match self.body(path, kv) {
            Ok(b) => b,
            Err(r) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), r.text().as_bytes());
                String::new()
            }
        }
    }

    /// `_doc_query PATH [key value]...`.
    pub fn query(&self, path: &str, kv: &[(&str, &str)]) -> String {
        let mut q: Vec<(String, String)> = Vec::new();
        if !self.ws.is_empty() {
            q.push(("workspace".into(), self.ws.clone()));
        }
        if !path.is_empty() {
            q.push(("path".into(), path.into()));
        }
        if !self.ob.is_empty() {
            q.push(("onBehalfOf".into(), self.ob.clone()));
        }
        q.extend(
            kv.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.to_string(), v.to_string())),
        );
        urlencode(&q)
    }

    /// `_doc_print_json_or SCRIPT`: the body as it came with `--json`,
    /// else the reader's lines.
    pub fn json_or(&self, body: &[u8], reader: impl FnOnce(&mut Pr, &Value) -> R<Exit>) -> Exit {
        if self.json {
            let mut b = body.to_vec();
            b.push(b'\n');
            return match out::write_flushed(&mut std::io::stdout().lock(), &b) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        let mut pr = Pr::default();
        let r = py::load(body, 3).and_then(|v| reader(&mut pr, &v));
        pr.finish(r)
    }

    /// The help page, exit 0.
    pub fn page(&self, key: &str) -> Exit {
        match out::stdout(crate::text::help(key).unwrap_or_default()) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        }
    }

    fn dispatch(&mut self, words: &[String]) -> Step {
        let verb = words.first().map_or("", String::as_str);
        let rest = words.get(1..).unwrap_or_default();
        if verb == "cores" {
            return more::cores(self, rest);
        }
        let strs: Vec<&str> = words.iter().map(String::as_str).collect();
        if verb.is_empty() || wants_help(&strs) {
            return Ok(self.page("doc"));
        }
        if board::call(words) {
            return board::run(self, verb, rest);
        }
        match verb {
            "diff" => {
                if rest
                    .iter()
                    .any(|a| a == "--version" || a.starts_with("--version="))
                {
                    more::history_diff(self, rest)
                } else {
                    tool::tool(self, "diff", rest)
                }
            }
            "cat" | "head" | "wc" | "grep" | "sed" | "patch" | "apply" | "append" | "insert"
            | "replace" | "delete" => tool::tool(self, verb, rest),
            "layer" | "outline" | "node" | "comment" | "reply" | "resolve" | "comments"
            | "reanchor" | "clip" | "track" | "marker" | "media" | "timeline" | "feature"
            | "hole" | "object" | "scene" | "collection" | "entity" | "attrib" | "block"
            | "text" => tool::blob(self, verb, rest),
            "cad" => more::cad(self, rest),
            "av" => more::av(self, rest),
            "purge-comments" | "erase-participant" => print::owner_purge(self, verb, rest),
            "view" => print::view(self, rest),
            "lease" | "lock" | "unlock" => print::lease(self, verb, rest),
            "claim" => print::claim(self, rest),
            "activity" => print::activity(self, rest),
            "versions" | "show" | "restore" | "version" | "revert" => {
                more::history(self, verb, rest)
            }
            "tail" => {
                if rest.iter().any(|a| a == "-f" || a == "--follow") {
                    let r: Vec<String> = rest
                        .iter()
                        .filter(|a| *a != "-f" && *a != "--follow")
                        .cloned()
                        .collect();
                    let args = self.parse(&r)?;
                    more::watch(self, &args)
                } else {
                    tool::tool(self, "tail", rest)
                }
            }
            "watch" => {
                let args = self.parse(rest)?;
                more::watch(self, &args)
            }
            "who" => {
                let args = self.parse(rest)?;
                print::who(self, None, &args)
            }
            "follow" => {
                let args = self.parse(rest)?;
                if args.is_empty() {
                    return Err(self.usage(format!("{CLI_NAME} doc follow <name> <file.md>")));
                }
                let name = args[0].clone();
                print::who(self, Some(&name), &args)
            }
            "say" => print::say(self, rest),
            "cursor" => print::cursor(self, rest),
            "write" => tool::write(self, rest),
            "undo" | "redo" => print::undo_redo(self, verb, rest),
            "log" => print::log(self, rest),
            "status" => print::status(self, rest),
            "flush" | "open" | "leave" => print::flush_open_leave(self, verb, rest),
            "approvals" => print::approvals(self, rest),
            "approve" | "decline" => print::approve(self, verb, rest),
            "share" | "unshare" => print::share(self, verb, rest),
            "shares" => print::shares(self, rest),
            "blame" => print::blame(self, rest),
            "mv" => print::mv(self, rest),
            "export" => more::export(self, rest),
            "help" => Ok(self.page("doc")),
            v => {
                if v.starts_with('-') || v.chars().any(|c| !(c.is_ascii_lowercase() || c == '-')) {
                    return Err(self.usage(format!(
                        "unknown {CLI_NAME} doc verb: {v} ({CLI_NAME} doc --help)"
                    )));
                }
                tool::tool(self, v, rest)
            }
        }
    }
}

/// Python's `UnicodeDecodeError` line for bytes that aren't UTF-8.
pub fn utf8_raise(line: u32, bytes: &[u8], e: std::str::Utf8Error) -> Raise {
    let at = e.valid_up_to();
    let b = bytes[at];
    let why = match e.error_len() {
        Some(_) if b >= 0xF5 || (0x80..0xC2).contains(&b) => "invalid start byte",
        Some(_) => "invalid continuation byte",
        None => "unexpected end of data",
    };
    let end = match e.error_len() {
        Some(n) if n > 1 => format!("in position {at}-{}", at + n - 1),
        None if bytes.len() - at > 1 => format!("in position {at}-{}", bytes.len() - 1),
        _ => format!("in position {at}"),
    };
    Raise::at(
        line,
        format!("UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{b:02x} {end}: {why}"),
    )
}

/// `_doc_fail STATUS BODY`: the refusal's lines and the doc exit code.
pub fn fail(status: &str, body: &[u8], as_json: bool) -> Exit {
    let mut pr = Pr::default();
    let r = fail_lines(&mut pr, status, body, as_json);
    pr.finish(r)
}

fn fail_lines(pr: &mut Pr, status: &str, body: &[u8], as_json: bool) -> R<Exit> {
    let raw = py::stdin_text(body, 4)?.into_owned();
    let mut code = s("");
    let mut hint = s("");
    let v = crate::out::pylib::json::loads(&raw).ok();
    match &v {
        Some(Value::Object(pairs)) => match get(pairs, "error") {
            Some(e @ Value::Object(ep)) => {
                code = get(ep, "code")
                    .filter(|c| opy::truthy(c))
                    .cloned()
                    .unwrap_or(s(""));
                hint = get(ep, "hint")
                    .filter(|h| opy::truthy(h))
                    .or_else(|| get(ep, "message").filter(|m| opy::truthy(m)))
                    .cloned()
                    .unwrap_or(s(""));
                let _ = e;
            }
            Some(Value::Null) | None => {}
            Some(e) => hint = s(&opy::str_of(e)),
        },
        _ => {
            let stripped = raw.trim_matches(py_space);
            if !stripped.is_empty() {
                hint = s(&stripped.chars().take(300).collect::<String>());
            }
        }
    }
    if status == "000" {
        code = s("daemon_unreachable");
        hint = s(&format!("no answer from the {PRODUCT_NAME} daemon"));
    }
    let mut rc: Exit = match status {
        "400" | "405" | "413" | "429" => Exit::Usage,
        "401" | "403" => Exit::Refused,
        "404" | "422" => Exit::NotFound,
        _ => Exit::Error,
    };
    if status == "409" {
        rc = match code.clone() {
            Value::Str(t) => match t.as_str().unwrap_or("") {
                "not_joined" | "bad_approval" | "held" => Exit::Usage,
                "pack_mismatch" => Exit::NotFound,
                "revert_too_old" | "base_expired" => Exit::TooOld,
                _ => Exit::Error,
            },
            _ => Exit::Error,
        };
    }
    if let Some(Value::Object(pairs)) = &v {
        if let Some(Value::Object(ep)) = get(pairs, "error") {
            if let Some(x @ (Value::Int(_) | Value::Bool(_))) = get(ep, "exit") {
                rc = py::exit_of(x);
            }
        }
    }
    if !opy::truthy(&hint) {
        hint = s(&format!("the daemon refused (HTTP {status})"));
    }
    if !as_json {
        pr.eline(&format!("Error: {}", opy::str_of(&hint)));
    }
    let code = if opy::truthy(&code) { code } else { s("error") };
    let st = if !status.is_empty() && status.bytes().all(|b| b.is_ascii_digit()) {
        Value::Int(status.trim_start_matches('0').to_string()).normal_int()
    } else {
        Value::Int("0".into())
    };
    pr.eline(&dumps(&obj(vec![(
        "error",
        obj(vec![("code", code), ("hint", hint), ("status", st)]),
    )])));
    if let (Some(Value::Object(pairs)), false) = (&v, as_json) {
        if let Some(Value::Array(cs)) = get(pairs, "conflicts") {
            for c in cs {
                conflict(pr, c)?;
            }
        }
    }
    Ok(rc)
}

fn conflict(pr: &mut Pr, c: &Value) -> R<()> {
    let ln = py::gt_or(c, "lines", 84)?
        .cloned()
        .unwrap_or(Value::Array(vec![s("?"), s("?")]));
    let yl = py::gt(c, "yourLines", 85)?;
    let cl = py::gt(c, "currentLines", 85)?;
    let two = |x: Option<&Value>| matches!(x, Some(Value::Array(a)) if a.len() == 2);
    let ours = opy::repr(py::gt(c, "ours", 88)?.unwrap_or(&Value::Null));
    if two(yl) && two(cl) {
        let (yl, cl) = (yl.unwrap_or(&Value::Null), cl.unwrap_or(&Value::Null));
        let theirs = opy::repr(py::gt(c, "theirs", 88)?.unwrap_or(&Value::Null));
        pr.eline(&format!(
            "conflict at your lines {}-{} (now lines {}-{}):\n  yours: {ours}\n  now:   {theirs}",
            py::st(&py::at(yl, 0, 88)?),
            py::st(&py::at(yl, 1, 88)?),
            py::st(&py::at(cl, 0, 88)?),
            py::st(&py::at(cl, 1, 88)?)
        ));
    } else {
        let a = py::st(&py::at(&ln, 0, 90)?);
        let b = py::st(&py::at(&ln, 1, 90)?);
        let theirs = opy::repr(py::gt(c, "theirs", 90)?.unwrap_or(&Value::Null));
        pr.eline(&format!(
            "conflict at lines {a}-{b}:\n  yours:  {ours}\n  theirs: {theirs}"
        ));
    }
    Ok(())
}

trait NormalInt {
    fn normal_int(self) -> Value;
}

impl NormalInt for Value {
    /// `int("007")` is 7; `int("000")` is 0.
    fn normal_int(self) -> Value {
        match self {
            Value::Int(t) if t.is_empty() => Value::Int("0".into()),
            v => v,
        }
    }
}
