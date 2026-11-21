//! `k5 migrate`, as the shell CLI's `cmd_migrate` answers it
//! (`cli/src/migrate.sh`): a box move that carries the vault. Every form:
//! the help, `status`, `import-secrets begin`, `import-secrets <file>`
//! (the dry run) and `--apply`, `import-secrets --undo`, `export-secrets
//! --to … --out …` and `--unfreeze`, and `approve mv_…`, each with
//! `--json`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), then the virtual desktop's guard
//!    and the connection gate ([`crate::prelude::gate`]). `migrate` alone,
//!    `help`, `-h`, `--help`, or `-h`/`--help` anywhere after the verb skip
//!    the connection gate ([`skips_gate`]); `-h` or `--help` anywhere skips
//!    the guard.
//! 2. `migrate` is an `id` tool, so with `HOOK_SOCK` set: the
//!    stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]), the help too.
//! 3. The words ([`parse`]): the help, a `usage:` line (exit 2), or the
//!    step to send. A value flag with no word after it is the shell's
//!    `_need_value` line, exit 2.
//! 4. `_migrate_send` ([`send`]): inside a K2 terminal, or with no vault
//!    socket, `_sidecar_send` (the cell socket with the passport, else
//!    TCP); else the vault socket, the person's channel
//!    ([`confirm::migrate_sock_call`]).
//! 5. `_migrate_step` ([`step`]): a 403 that asks for the vault confirm is
//!    confirmed over the vault socket ([`confirm::confirm_token`]) and the
//!    step is sent again with its token. A confirm that fails is exit 3.
//! 6. `_migrate_finish` ([`finish`]): a 202 is held (its `HELD:` hint,
//!    exit 2); any other status that is not 200 is `_vault_fail`
//!    ([`confirm::fail`]); `--json` prints the body as it came; else the
//!    shell's Python lays it out ([`render`]).
//! 7. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell CLI answers ([`run`] returns `None`): a word that is not
//! UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported, a `python3`
//! the binary can't tell, and a form that asks the daemon with a port that
//! is not one curl takes (a form past the connection gate only when `-h`
//! or `--help` is a flag's value, or a port that is set and no port).
//!
//! Where the shell's Python raises (an answer that is not JSON, a field of
//! a type it doesn't expect), this prints a Python 3.12 traceback and
//! exits 1, as the shell's `set -e` does.

use super::{curl_port, ljust, py_get, py_iter, raised, Raise, Shown};
use crate::confirm::{self, Confirm, Split};
use crate::daemon;
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::ops;
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::Client;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::PathBuf;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["migrate"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "migrate";
/// The routes, under this.
pub const ROUTE: &str = "/cli/migrate/secrets/";

/// `approve -h`.
pub const APPROVE_HELP: &str = concat!(
    cli_name!(),
    " migrate approve <mv_\u{2026}> \u{2014} run a held move step (box owner, own terminal; asks the vault confirm)"
);
/// `approve` with a flag it doesn't have.
pub const APPROVE_FLAG: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate approve <mv_\u{2026}> [--json]"
);
/// `approve` with no id.
pub const APPROVE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate approve <mv_\u{2026}> (",
    cli_name!(),
    " migrate status lists held steps)"
);
/// `export-secrets` with a word it doesn't take.
pub const EXPORT_FLAG: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate export-secrets --to <age1\u{2026}> --out <file> | --unfreeze (",
    cli_name!(),
    " migrate --help)"
);
/// `export-secrets` without `--to` or `--out`.
pub const EXPORT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate export-secrets --to <age1\u{2026} from the new box> --out <file>"
);
/// `import-secrets` with a flag it doesn't have.
pub const IMPORT_FLAG: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate import-secrets begin | <file> [--apply] | --undo (",
    cli_name!(),
    " migrate --help)"
);
/// `import-secrets` with no file.
pub const IMPORT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate import-secrets <file> [--apply] (or: begin, --undo)"
);
/// A verb `migrate` doesn't have.
pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " migrate status | import-secrets \u{2026} | export-secrets \u{2026} | approve mv_\u{2026} (",
    cli_name!(),
    " migrate --help)"
);
/// `_migrate_finish` with no answer.
pub const UNAVAILABLE: &str = concat!(
    "vault_unavailable: the ",
    k2_client::product_name!(),
    " daemon didn't answer (is it running on this machine?)"
);

/// How an answer is laid out: `_migrate_finish`'s second word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Render {
    /// `approve`: by the answer's keys.
    Plain,
    Status,
    Begin,
    Export,
    Unfreeze,
    Dry,
    Import,
    Undo,
}

/// One request: `_migrate_send` alone, or a `_migrate_step`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// `true`: a POST.
    pub post: bool,
    pub route: &'static str,
    pub payload: String,
    /// A `_migrate_step`: a 403 that asks for the vault confirm is
    /// confirmed and sent again.
    pub step: bool,
    pub json: bool,
    pub render: Render,
}

/// What `cmd_migrate` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_migrate_help`, exit 0.
    Help,
    /// `approve -h`: one line, exit 0.
    ApproveHelp,
    /// A `usage:` line, exit 2.
    Usage(&'static str),
    /// A value flag last on the line (the flag): the shell's
    /// `_need_value` line, exit 2.
    ShiftFails(String),
    /// A request.
    Call(Call),
}

/// `json.dumps` of an object of strings and booleans.
fn body(pairs: Vec<(&str, Value)>) -> String {
    confirm::dumps(&confirm::object(pairs))
}

fn st(s: &str) -> Value {
    Value::Str(Text::from(s))
}

/// `case "$p" in /*) ;; *) p="$(pwd)/$p" ;; esac`.
fn absolute(p: &str, pwd: &str) -> String {
    if p.starts_with('/') {
        p.to_string()
    } else {
        format!("{pwd}/{p}")
    }
}

/// `cmd_migrate`'s words (after `migrate`); `pwd` is the shell's `$(pwd)`,
/// asked only for a relative file.
pub fn parse(words: &[&str], pwd: &dyn Fn() -> String) -> Parsed {
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match sub {
        "" | "-h" | "--help" | "help" => Parsed::Help,
        "status" => Parsed::Call(Call {
            post: false,
            route: "status",
            payload: String::new(),
            step: false,
            json: rest.first() == Some(&"--json"),
            render: Render::Status,
        }),
        "approve" => {
            let (mut id, mut json) = ("", false);
            for w in rest {
                match *w {
                    "--json" => json = true,
                    "-h" | "--help" => return Parsed::ApproveHelp,
                    w if w.starts_with('-') => return Parsed::Usage(APPROVE_FLAG),
                    w => id = w,
                }
            }
            if id.is_empty() {
                return Parsed::Usage(APPROVE_USAGE);
            }
            Parsed::Call(Call {
                post: true,
                route: "approve",
                payload: body(vec![("request", st(id))]),
                step: true,
                json,
                render: Render::Plain,
            })
        }
        "export-secrets" => {
            let (mut to, mut out, mut unfreeze, mut json) = ("", "", false, false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--to" | "--out" | "-o" => {
                        let Some(v) = rest.get(i + 1) else {
                            return Parsed::ShiftFails(rest[i].to_string());
                        };
                        if rest[i] == "--to" {
                            to = v;
                        } else {
                            out = v;
                        }
                        i += 2;
                        continue;
                    }
                    "--unfreeze" => unfreeze = true,
                    "--json" => json = true,
                    "-h" | "--help" => return Parsed::Help,
                    _ => return Parsed::Usage(EXPORT_FLAG),
                }
                i += 1;
            }
            if unfreeze {
                return Parsed::Call(Call {
                    post: true,
                    route: "unfreeze",
                    payload: "{}".into(),
                    step: true,
                    json,
                    render: Render::Unfreeze,
                });
            }
            if to.is_empty() || out.is_empty() {
                return Parsed::Usage(EXPORT_USAGE);
            }
            let out = absolute(out, &pwd());
            Parsed::Call(Call {
                post: true,
                route: "export",
                payload: body(vec![("to", st(to)), ("out", st(&out))]),
                step: true,
                json,
                render: Render::Export,
            })
        }
        "import-secrets" => {
            let (mut file, mut apply, mut undo, mut new, mut json, mut begin) =
                ("", false, false, false, false, false);
            for w in rest {
                match *w {
                    "begin" => begin = true,
                    "--new" => new = true,
                    "--apply" => apply = true,
                    "--undo" => undo = true,
                    "--json" => json = true,
                    "-h" | "--help" => return Parsed::Help,
                    w if w.starts_with('-') => return Parsed::Usage(IMPORT_FLAG),
                    w => file = w,
                }
            }
            if begin {
                return Parsed::Call(Call {
                    post: true,
                    route: "begin",
                    payload: body(vec![("new", Value::Bool(new))]),
                    step: false,
                    json,
                    render: Render::Begin,
                });
            }
            if undo {
                return Parsed::Call(Call {
                    post: true,
                    route: "undo",
                    payload: "{}".into(),
                    step: true,
                    json,
                    render: Render::Undo,
                });
            }
            if file.is_empty() {
                return Parsed::Usage(IMPORT_USAGE);
            }
            let file = absolute(file, &pwd());
            Parsed::Call(Call {
                post: true,
                route: "import",
                payload: body(vec![("file", st(&file)), ("apply", Value::Bool(apply))]),
                step: apply,
                json,
                render: if apply { Render::Import } else { Render::Dry },
            })
        }
        _ => Parsed::Usage(USAGE),
    }
}

/// The shell skips its connection gate (`cli/src/cli-tools-policy.sh`):
/// `migrate` alone, `help`, `-h`, `--help`, or `-h`/`--help` anywhere
/// after the verb.
pub fn skips_gate(words: &[&str]) -> bool {
    match words.first() {
        None | Some(&"" | &"help" | &"--help" | &"-h") => true,
        Some(_) => words[1..].iter().any(|w| matches!(*w, "-h" | "--help")),
    }
}

/// `$PWD` as the shell has it.
fn shell_pwd() -> String {
    daemon::logical_cwd(std::env::var_os("PWD"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let words = all.get(1..).unwrap_or_default();
    if !python::family_native(python::here()) {
        return None;
    }
    let parsed = parse(words, &shell_pwd);
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let skip = skips_gate(words);
    let asks = matches!(parsed, Parsed::Call(_));
    let port = settings.port_text.as_deref().and_then(curl_port);
    let gate = prelude::gate(&settings);
    if asks && port.is_none() && (gate.is_none() || skip) {
        return None;
    }
    let offline = all.iter().any(|w| matches!(*w, "-h" | "--help"));
    match gate {
        Some(Gate::VirtualDesktop) if !offline => return Some(Gate::VirtualDesktop.stop()),
        Some(_) if !skip => return Some(Gate::CannotConnect.stop()),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    let call = match parsed {
        Parsed::Help => {
            return Some(Shown::out(crate::text::help("migrate").unwrap_or_default()).print())
        }
        Parsed::ApproveHelp => {
            return Some(match out::stdout_line(APPROVE_HELP) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            })
        }
        Parsed::Usage(line) => {
            out::stderr_line(line);
            return Some(Exit::Usage);
        }
        Parsed::ShiftFails(flag) => return Some(out::stop_need_value(&flag)),
        Parsed::Call(call) => call,
    };
    let mut settings = settings;
    settings.port = port;
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let mut cx = Cx {
        client,
        closing: Closing::default(),
        sock: confirm::sock_path(&settings.home),
    };
    let exit = match step(&mut cx, &call) {
        Ok(e) | Err(e) => e,
    };
    cx.closing.flush(&mut cx.client, exit);
    Some(exit)
}

/// What a call works with.
pub struct Cx {
    pub client: Client,
    pub closing: Closing,
    /// `<K2 home>/vault.sock`.
    pub sock: PathBuf,
}

impl Cx {
    fn passport(&self) -> bool {
        self.client.passport().is_some()
    }
}

/// `_migrate_send METHOD <route> <payload>`. `Err`: the transport's own
/// end (its line printed, exit 1).
pub fn send(cx: &mut Cx, post: bool, route: &str, payload: &str) -> Result<Split, Exit> {
    if confirm::in_k2_terminal() || !super::live_socket(Some(&cx.sock)) {
        return confirm::sidecar_call(&mut cx.client, post, &format!("{ROUTE}{route}"), payload);
    }
    let sock = cx.sock.clone();
    Ok(confirm::migrate_sock_call(
        &mut cx.client,
        &sock,
        post,
        route,
        payload.as_bytes(),
    ))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `(json.load(sys.stdin).get("confirm") or {}).get(key)`, as the shell's
/// one-liner prints it into `$(…)`.
fn confirm_field(body: &[u8], key: &str) -> Result<Option<Value>, Raise> {
    let v = confirm::load(body)?;
    let c = or_object(py_get(&v, "confirm", 1)?);
    Ok(py_get(&c, key, 1)?.cloned())
}

/// What `print(x or "")` leaves in `$(…)`.
fn printed(v: Option<Value>) -> String {
    match v {
        Some(x) if py::truthy(&x) => py::str_of(&x).trim_end_matches('\n').to_string(),
        _ => String::new(),
    }
}

/// `_migrate_step`, or `_migrate_send` and `_migrate_finish` for a call
/// that is no step.
pub fn step(cx: &mut Cx, call: &Call) -> Result<Exit, Exit> {
    let mut s = send(cx, call.post, call.route, &call.payload)?;
    if call.step
        && s.status == "403"
        && (contains(&s.body, b"\"vault_confirm_required\"")
            || contains(&s.body, b"\"vault_presence_required\""))
    {
        // `binding="$(… python3 …)"` under `set -e`: the first one that
        // raises ends the command; the others read the same body.
        let binding = match confirm_field(&s.body, "secretId") {
            Ok(v) => printed(v),
            Err(r) => return Err(Shown::err(r.text(), Exit::Error).print()),
        };
        let label = printed(confirm_field(&s.body, "label").unwrap_or(None));
        let action = match confirm_field(&s.body, "action").unwrap_or(None) {
            Some(Value::Str(t)) if t == Text::from("box-move-export") => "box-move-export",
            _ => "vault-move",
        };
        if !binding.is_empty() {
            let passport = cx.passport();
            let mut c = Confirm {
                client: &mut cx.client,
                sock: &cx.sock,
                closing: &mut cx.closing,
                passport,
            };
            let Some(tok) = confirm::confirm_token(&mut c, action, &binding, &label) else {
                return Err(Exit::Refused);
            };
            let payload = confirm::with_confirm_token(&call.payload, &tok, None)
                .unwrap_or_else(|| call.payload.clone());
            drop(tok);
            s = send(cx, call.post, call.route, &payload)?;
        }
    }
    Ok(finish(cx, &s, call.json, call.render))
}

/// `printf '%s\n' "$_SC_BODY"`.
fn print_body(body: &[u8]) -> Result<(), Exit> {
    let mut text = body.to_vec();
    text.push(b'\n');
    out::write_flushed(&mut std::io::stdout().lock(), &text)
        .map_err(|e| out::after_stdout_error(&e))
}

/// `_migrate_finish <as_json> <render>`.
pub fn finish(cx: &mut Cx, s: &Split, as_json: bool, render: Render) -> Exit {
    if s.status == "202" {
        if as_json {
            if let Err(e) = print_body(&s.body) {
                return e;
            }
        } else {
            match held_line(&s.body) {
                Ok(line) => out::stderr_line(&line),
                Err(r) => return Shown::err(r.text(), Exit::Error).print(),
            }
        }
        return Exit::Usage;
    }
    if s.status != "200" {
        if as_json {
            return print_body(&s.body).map_or_else(|e| e, |()| confirm::exit_for(&s.status));
        }
        if s.status == "000" {
            out::stderr_line(UNAVAILABLE);
            return Exit::Error;
        }
        let passport = cx.passport();
        return confirm::fail(s, &mut cx.closing, passport);
    }
    if as_json {
        return print_body(&s.body).map_or_else(|e| e, |()| Exit::Ok);
    }
    render_body(&s.body, render).print()
}

/// `print("HELD: " + v.get("hint",""))`.
fn held_line(body: &[u8]) -> Result<String, Raise> {
    let v = confirm::load(body)?;
    let hint = py_get(&v, "hint", 1)?.cloned().unwrap_or_else(|| st(""));
    plus("HELD: ", &hint, 1)
}

// The shell's Python, line by line. Its lines are counted from the `-c`
// text's first (empty) line, for the traceback.

type R<T> = Result<T, Raise>;

/// `x or {}`.
fn or_object(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Object(Vec::new()),
    }
}

/// `x or ""`.
fn or_str(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => st(""),
    }
}

/// `"%s" % x` of a `.get` that may be missing.
fn ps(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

/// `prefix + x`.
fn plus(prefix: &str, x: &Value, line: u32) -> R<String> {
    match x {
        Value::Str(_) => Ok(format!("{prefix}{}", py::str_of(x))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `key(k)`, called from line `from`.
fn key(k: Option<&Value>, from: u32) -> R<String> {
    let k = or_object(k);
    let g = |name| py_get(&k, name, 10).map_err(|r| inside(from, r, "key"));
    Ok(format!(
        "{}  (fingerprint {}, expires {})",
        ps(g("recipient")?),
        ps(g("fingerprint")?),
        ps(g("expiresAt")?)
    ))
}

/// A raise inside a function called from line `from`.
fn inside(from: u32, r: Raise, name: &'static str) -> Raise {
    let line = r.frames.last().map_or(1, |f| f.0);
    Raise::inside(from, line, name, r.exception)
}

/// `creds(v)`, called from line `from`.
fn creds(out: &mut String, v: &Value, from: u32) -> R<()> {
    let c = match py_get(v, "systemdCreds", 12).map_err(|r| inside(from, r, "creds"))? {
        Some(x) if py::truthy(x) => x.clone(),
        _ => return Ok(()),
    };
    let items = py_iter(&c, 14).map_err(|r| inside(from, r, "creds"))?;
    let mut parts = Vec::new();
    for x in &items {
        let g = |name| py_get(x, name, 14).map_err(|r| inside(from, r, "creds"));
        parts.push(format!("{} ({})", ps(g("secret")?), ps(g("unit")?)));
    }
    out.push_str(&format!("re-place:     {}\n", parts.join(", ")));
    out.push_str(&format!(
        "              {CLI} vault systemd-cred NAME --replace, for each\n"
    ));
    Ok(())
}

/// `_migrate_finish`'s Python on a 200 answer.
pub fn render_body(body: &[u8], render: Render) -> Shown {
    let mut out = String::new();
    match render_into(&mut out, body, render) {
        Ok(()) => Shown::out(out),
        Err(r) => raised(out, String::new(), &r),
    }
}

fn render_into(out: &mut String, body: &[u8], render: Render) -> R<()> {
    let v = confirm::load(body).map_err(|r| Raise::at(3, r.exception))?;
    let mut kind = render;
    if kind == Render::Plain {
        kind = if ops::contains(&v, "out", 7)? {
            Render::Export
        } else if ops::contains(&v, "wasFrozen", 7)? {
            Render::Unfreeze
        } else if ops::contains(&v, "undone", 7)? {
            Render::Undo
        } else if ops::contains(&v, "replaced", 7)? {
            Render::Import
        } else {
            Render::Plain
        };
    }
    macro_rules! say {
        ($e:expr) => {{
            let l: String = $e;
            out.push_str(&l);
            out.push('\n');
        }};
    }
    let g = |name: &str, at: u32| py_get(&v, name, at);
    match kind {
        Render::Begin => {
            let lead = if g("created", 17)?.is_some_and(py::truthy) {
                "new move key: "
            } else {
                "move key:     "
            };
            say!(format!("{lead}{}", key(g("moveKey", 17)?, 17)?));
            let mk = or_object(g("moveKey", 18)?);
            say!(format!(
                "next:         on the OLD box: {CLI} migrate export-secrets --to {} --out ~/move.k2mv",
                ps(py_get(&mk, "recipient", 18)?)
            ));
        }
        Render::Export => {
            say!(format!(
                "exported:     {} ({} bytes) for move key {}",
                ps(g("out", 20)?),
                ps(g("bytes", 20)?),
                ps(g("moveKey", 20)?)
            ));
            let s = or_object(g("secrets", 21)?);
            say!(format!(
                "vault:        {} gen {}, {} yours / {} {} system secrets",
                ps(g("vaultId", 22)?),
                ps(g("gen", 22)?),
                ps(py_get(&s, "user", 22)?),
                ps(py_get(&s, "system", 22)?),
                k2_client::brand::PRODUCT_NAME
            ));
            say!("FROZEN:       this vault reads but refuses writes until the move is done or called off".into());
            say!(plus("next:         ", &or_str(g("next", 24)?), 24)?);
        }
        Render::Unfreeze => {
            say!(if g("wasFrozen", 26)?.is_some_and(py::truthy) {
                "unfrozen:     writes work again; the move key holder is gone".into()
            } else {
                "not frozen".into()
            });
        }
        Render::Dry => {
            let s = or_object(g("secrets", 28)?);
            let src = or_object(g("source", 29)?);
            say!("DRY RUN \u{2014} nothing changed".into());
            say!(format!(
                "vault:        {} gen {}, {} yours / {} {} system secrets",
                ps(g("vaultId", 31)?),
                ps(g("gen", 31)?),
                ps(py_get(&s, "user", 31)?),
                ps(py_get(&s, "system", 31)?),
                k2_client::brand::PRODUCT_NAME
            ));
            let host = or_str(py_get(&src, "host", 32)?);
            let host = if py::truthy(&host) {
                py::str_of(&host)
            } else {
                "?".into()
            };
            let skh = ps(g("sourceKeyHome", 32)?);
            let tunnel = match py_get(&src, "tunnelSubdomain", 32)? {
                Some(t) if py::truthy(t) => plus(", tunnel ", t, 32)?,
                _ => String::new(),
            };
            say!(format!(
                "from:         {host} ({skh}{tunnel}), exported {}",
                ps(g("exportedAt", 32)?)
            ));
            say!(format!("tunnel token: {}", ps(g("tunnelToken", 33)?)));
            creds(out, &v, 34)?;
            let r = or_object(g("replaces", 35)?);
            let home = k2_client::paths::HOME_DIR_NAME;
            if py_get(&r, "open", 36)?.is_some_and(py::truthy) {
                say!(format!(
                    "replaces:     this box\u{2019}s vault {} ({} yours); it is kept in ~/{home}/vault/replaced-<time>/",
                    ps(py_get(&r, "vaultId", 37)?),
                    ps(py_get(&r, "userSecrets", 37)?)
                ));
            } else {
                say!(format!(
                    "replaces:     this box\u{2019}s closed vault ({}); kept in ~/{home}/vault/replaced-<time>/",
                    ps(py_get(&r, "error", 39)?)
                ));
            }
            say!(plus("next:         ", &or_str(g("next", 40)?), 40)?);
        }
        Render::Import => {
            let s = or_object(g("secrets", 42)?);
            let rollback = if g("rollback", 43)?.is_some_and(py::truthy) {
                "yes"
            } else {
                "no"
            };
            say!(format!(
                "imported:     {} gen {}, key home {}, rollback {rollback}",
                ps(g("vaultId", 43)?),
                ps(g("gen", 43)?),
                ps(g("keyHome", 43)?)
            ));
            say!(format!(
                "secrets:      {} yours / {} {} system",
                ps(py_get(&s, "user", 44)?),
                ps(py_get(&s, "system", 44)?),
                k2_client::brand::PRODUCT_NAME
            ));
            say!(format!("tunnel token: {}", ps(g("tunnelToken", 45)?)));
            creds(out, &v, 46)?;
            say!(format!("replaced:     {}", ps(g("replaced", 47)?)));
            let next = match g("next", 48)? {
                Some(x) if py::truthy(x) => py_iter(x, 48)?,
                _ => Vec::new(),
            };
            for n in &next {
                say!(plus("next:         ", n, 49)?);
            }
        }
        Render::Undo => {
            let vv = or_object(g("vault", 51)?);
            let state = if py_get(&vv, "open", 52)?.is_some_and(py::truthy) {
                "open".to_string()
            } else {
                format!("closed: {}", ps(py_get(&vv, "error", 52)?))
            };
            say!(format!(
                "undone:       the import of {}; the replaced vault is back ({state})",
                ps(g("undone", 52)?)
            ));
            say!(py::str_of(&or_str(g("note", 53)?)));
        }
        Render::Status => {
            let vv = or_object(g("vault", 55)?);
            if py_get(&vv, "open", 56)?.is_some_and(py::truthy) {
                say!(format!(
                    "vault:        open ({}, key home {})",
                    ps(py_get(&vv, "vaultId", 57)?),
                    ps(py_get(&vv, "keyHome", 57)?)
                ));
            } else {
                let e = or_object(py_get(&vv, "error", 59)?);
                say!(format!(
                    "vault:        CLOSED \u{2014} {}: {}",
                    ps(py_get(&e, "code", 60)?),
                    ps(py_get(&e, "hint", 60)?)
                ));
            }
            let frozen = match g("frozen", 61)? {
                Some(f) if py::truthy(f) => format!(
                    "YES since {} for move key {} ({CLI} migrate export-secrets --unfreeze to call it off)",
                    ps(py_get(f, "at", 62)?),
                    ps(py_get(f, "moveKey", 62)?)
                ),
                _ => "no".into(),
            };
            say!(format!("frozen:       {frozen}"));
            let mk = match g("moveKey", 63)? {
                Some(k) if py::truthy(k) => key(Some(k), 63)?,
                _ => format!("none ({CLI} migrate import-secrets begin)"),
            };
            say!(format!("move key:     {mk}"));
            let last = match g("lastImport", 64)? {
                Some(li) if py::truthy(li) => format!(
                    "{} at {} (undo: {CLI} migrate import-secrets --undo)",
                    ps(py_get(li, "vaultId", 65)?),
                    ps(py_get(li, "at", 65)?)
                ),
                _ => "none".into(),
            };
            say!(format!("last import:  {last}"));
            let held = match g("held", 66)? {
                Some(x) if py::truthy(x) => py_iter(x, 66)?,
                _ => Vec::new(),
            };
            for h in &held {
                let hg = |name| py_get(h, name, 67);
                say!(format!(
                    "held:         {} {} {} by {}, expires {}",
                    ps(hg("id")?),
                    ljust(&ps(hg("step")?), 8),
                    ljust(&ps(hg("status")?), 8),
                    ps(hg("by")?),
                    ps(hg("expiresAt")?)
                ));
            }
        }
        Render::Plain => {
            let opts = Options {
                indent: Some(2),
                ..Options::default()
            };
            say!(json::to_string(&v, &opts).unwrap_or_default());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pwd() -> String {
        "/w".into()
    }

    #[test]
    fn the_words_are_the_shell_s() {
        assert_eq!(parse(&[], &pwd), Parsed::Help);
        assert_eq!(parse(&["approve", "-h"], &pwd), Parsed::ApproveHelp);
        assert_eq!(
            parse(&["approve", "--x", "-h"], &pwd),
            Parsed::Usage(APPROVE_FLAG)
        );
        assert_eq!(parse(&["approve"], &pwd), Parsed::Usage(APPROVE_USAGE));
        assert_eq!(
            parse(&["export-secrets", "--to"], &pwd),
            Parsed::ShiftFails("--to".into())
        );
        assert_eq!(parse(&["nope"], &pwd), Parsed::Usage(USAGE));
        let Parsed::Call(c) = parse(&["export-secrets", "--to", "age1x", "-o", "m.k2mv"], &pwd)
        else {
            panic!("a call")
        };
        assert_eq!(c.payload, r#"{"to": "age1x", "out": "/w/m.k2mv"}"#);
        let Parsed::Call(c) = parse(&["import-secrets", "begin", "--new", "--json"], &pwd) else {
            panic!("a call")
        };
        assert_eq!(
            (c.payload.as_str(), c.json, c.step),
            (r#"{"new": true}"#, true, false)
        );
    }

    #[test]
    fn the_gate_skips_are_the_shell_s() {
        assert!(skips_gate(&[]));
        assert!(skips_gate(&["help"]));
        assert!(skips_gate(&["status", "-h"]));
        assert!(!skips_gate(&["status"]));
        assert!(!skips_gate(&["--json"]));
    }
}
