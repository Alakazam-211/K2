//! `k5 send-feedback`: feedback about the product to its makers, as the
//! shell CLI's `cmd_send_feedback` (`cli/src/send-feedback.sh`).
//!
//! 1. The settings and the gates ([`cp::open`]). `send-feedback` alone (or
//!    with an empty first word) and `help` need no daemon: only the
//!    virtual desktop's guard stops them. It is an open tool.
//! 2. The verb:
//!    - none: the help on stderr, exit 2; `help`: the help.
//!    - `list [--state S] [--json]`, `show <id> [--json]`, `retry <id>`,
//!      `cancel <id>`: one request each.
//!    - `settings [--enabled on|off] [--agents off|cli|all] [--attach
//!      <parts>] [--agent-diagnostics on|off] [--json]`: a read, or a
//!      write (after a read that tells an older server apart when the
//!      words need a 0.46.17 one), printed by the shell's bash readers.
//!    - anything else is a report: its checks, then with `--diagnostics`
//!      (or `--diagnostics-preview`) the details shown and asked about on
//!      the terminal, the POST, and up to ten reads of its state, half a
//!      second apart, until it is no longer waiting.
//!
//!    Each request is `_sf_call` ([`sf_call`]): in a session (`HOOK_SOCK`
//!    set, no remote host) it carries the session's passport, never the
//!    owner token, also when the cell socket doesn't serve the route and
//!    it goes over TCP; a session with no passport is [`NO_PASSPORT`],
//!    exit 3, nothing sent. Then `_compute_call` ([`cp::call`]).
//! 3. The EXIT trap's lines ([`cp::close`]).
//!
//! `[[:space:]]` is read as the C locale reads it (ASCII white space), as
//! the parity rig runs the shell. `-h` and `--help` are the front's: the
//! shell answers them.

use super::app::call::{Cj, Ctx};
use super::app::py::{self as p, Out};
use super::approve::cp::{self, body_line, env_text, raised, read_one, show, stdout_line, Opened};
use super::{py_space, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["send-feedback"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

const PRODUCT: &str = product_name!();
/// The settings route (`setup agent-reports` asks it too).
pub(crate) const SETTINGS: &str = "/cli/product-feedback/settings";
const LIST_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " send-feedback list [--state S] [--json]"
);
const SHOW_USAGE: &str = concat!("usage: ", cli_name!(), " send-feedback show <id> [--json]");
const SETTINGS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " send-feedback settings [--enabled on|off] [--agents off|cli|all] [--attach <parts>|none|all] [--agent-diagnostics on|off] [--json]"
);

/// `_sf_session_token`'s line: a session with no passport.
pub const NO_PASSPORT: &str = concat!(
    "Error: in a session, ",
    cli_name!(),
    " send-feedback doesn't act as this server's owner, and this session has no passport, so nothing was sent."
);

/// `_sf_call <GET|POST> <path> [payload]`: `_sf_session_token`, then
/// `_compute_call`. In a session on this machine with no passport: its
/// line, exit 3, nothing sent. With one, the client's TCP credential is
/// the passport already (there is no owner token in it).
fn sf_call(ctx: &mut Ctx, post: bool, path: &str, payload: &str) -> Result<Vec<u8>, Exit> {
    let c = &ctx.client;
    if c.session && !c.force_tcp && c.passport().is_none() {
        out::stderr_line(NO_PASSPORT);
        return Err(Exit::Refused);
    }
    cp::call(ctx, post, path, payload)
}

/// `k5 send-feedback ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let first = words.first().copied().unwrap_or("");
    let local = matches!(first, "" | "help");
    let mut ctx = match cp::open(args, local) {
        Opened::Shell => return None,
        Opened::Stop(Exit::Ok) if local => {
            return Some(if first == "help" {
                match out::stdout(help()) {
                    Ok(()) => Exit::Ok,
                    Err(e) => out::after_stdout_error(&e),
                }
            } else {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), help().as_bytes());
                Exit::Usage
            });
        }
        Opened::Stop(e) => return Some(e),
        Opened::Ready(c, _) => c,
    };
    let rest = &words[1..];
    let exit = match first {
        "list" => list(&mut ctx, rest),
        "show" => show_one(&mut ctx, rest),
        "retry" | "cancel" => transition(&mut ctx, first, rest),
        "settings" => settings(&mut ctx, rest),
        _ => send(&mut ctx, &words),
    };
    Some(cp::close(ctx, exit))
}

/// `_send_feedback_usage`.
fn help() -> &'static str {
    crate::text::help("send-feedback").expect("the send-feedback help page is embedded")
}

fn usage(line: &str) -> Exit {
    out::stderr_line(line);
    Exit::Usage
}

/// `${s//[[:space:]]/}` is empty (the C locale's white space).
fn blank(s: &[u8]) -> bool {
    s.iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
}

/// What `_sf_send` read of its words.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Bytes: `-` reads it from stdin.
    pub text: Vec<u8>,
    pub category: String,
    pub scope: String,
    pub command: String,
    pub diag: bool,
    pub preview: bool,
    pub yes: bool,
    pub json: bool,
}

/// `_sf_send`'s words and checks, in its order: the report, or the
/// usage line (exit 2). `stdin` is read for a `-`.
pub fn parse_report(words: &[&str], stdin: &mut dyn FnMut() -> Vec<u8>) -> Result<Report, String> {
    let mut r = Report::default();
    let (mut have_text, mut have_scope, mut have_command) = (false, false, false);
    let mut i = 0;
    let one_text = |have: bool, line: &str| -> Result<(), String> {
        if have {
            Err(line.to_string())
        } else {
            Ok(())
        }
    };
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        match w {
            "--category" => {
                r.category = next
                    .ok_or("usage: --category bug|idea|praise|other")?
                    .into();
                i += 2;
            }
            "--scope" => {
                r.scope = next.ok_or("usage: --scope cli|platform")?.into();
                have_scope = true;
                i += 2;
            }
            "--command" => {
                r.command = next
                    .ok_or("usage: --command \"<the command you ran>\"")?
                    .into();
                have_command = true;
                i += 2;
            }
            "--diagnostics" => {
                r.diag = true;
                i += 1;
            }
            "--diagnostics-preview" => {
                r.preview = true;
                i += 1;
            }
            "--yes" | "-y" => {
                r.yes = true;
                i += 1;
            }
            "--json" => {
                r.json = true;
                i += 1;
            }
            "--" => {
                i += 1;
                let Some(t) = words.get(i) else {
                    break;
                };
                one_text(have_text, "usage: one text (quote it)")?;
                r.text = t.as_bytes().to_vec();
                have_text = true;
                i += 1;
            }
            "-" => {
                one_text(have_text, "usage: one text (quote it)")?;
                r.text = stdin();
                have_text = true;
                i += 1;
            }
            w if w.starts_with("--category=") => {
                r.category = w["--category=".len()..].into();
                i += 1;
            }
            w if w.starts_with("--scope=") => {
                r.scope = w["--scope=".len()..].into();
                have_scope = true;
                i += 1;
            }
            w if w.starts_with("--command=") => {
                r.command = w["--command=".len()..].into();
                have_command = true;
                i += 1;
            }
            w if w.starts_with('-') => {
                return Err(format!(
                    concat!("Unknown flag: {} (", cli_name!(), " send-feedback --help)"),
                    w
                ))
            }
            w => {
                one_text(
                    have_text,
                    concat!(
                        "usage: one text (quote it): ",
                        cli_name!(),
                        " send-feedback \"<text>\""
                    ),
                )?;
                r.text = w.as_bytes().to_vec();
                have_text = true;
                i += 1;
            }
        }
    }
    if r.diag && r.preview {
        return Err("usage: --diagnostics or --diagnostics-preview, not both".into());
    }
    if !r.category.is_empty() && !matches!(r.category.as_str(), "bug" | "idea" | "praise" | "other")
    {
        return Err("usage: --category is bug, idea, praise or other".into());
    }
    match r.scope.as_str() {
        "cli" | "platform" => {}
        "" if have_scope => return Err("usage: --scope cli|platform".into()),
        "" => {}
        _ => {
            return Err(format!(
                "usage: --scope is cli (a problem with a {PRODUCT} command) or platform (anything else about {PRODUCT})"
            ))
        }
    }
    if r.command.contains(['\n', '\r']) {
        return Err("usage: --command is the one line you ran, without a new line".into());
    }
    if have_command && blank(r.command.as_bytes()) {
        return Err("usage: --command needs the command you ran".into());
    }
    if r.scope == "cli" && !have_command {
        return Err(
            "usage: --scope cli names the command you ran: --command \"<the command>\"".into(),
        );
    }
    if !r.preview && blank(&r.text) {
        return Err(concat!(
            "usage: ",
            cli_name!(),
            " send-feedback \"<text>\" (or - for stdin). ",
            cli_name!(),
            " send-feedback --help"
        )
        .to_string());
    }
    Ok(r)
}

fn send(ctx: &mut Ctx, words: &[&str]) -> Exit {
    let r = match parse_report(words, &mut || {
        crate::prompt::read_all_stdin().expose().to_vec()
    }) {
        Ok(r) => r,
        Err(line) => return usage(&line),
    };
    let mut draft = String::new();
    if r.diag || r.preview {
        let d = Cj::new()
            .s("client", "cli")
            .s("page", "cli")
            .s("cliVersion", crate::embedded::version())
            .body();
        let preview = match sf_call(ctx, true, "/cli/product-feedback/diagnostics", &d) {
            Ok(b) => b,
            Err(e) => return e,
        };
        if r.preview && r.json {
            return body_line(&preview);
        }
        out::stderr_line(&format!(
            "These details about {PRODUCT} on this computer would be sent, exactly as below:"
        ));
        out::stderr_line("");
        if let Err(e) = show(&sections(&preview)) {
            return e;
        }
        if r.preview {
            out::stderr_line("Nothing was sent.");
            return Exit::Ok;
        }
        let answer = if r.yes {
            "y".to_string()
        } else {
            match ask_tty("Send these details with your feedback? [y/N] ") {
                Some(a) => a,
                None => {
                    return usage("usage: --diagnostics asks before sending, and there is no terminal to ask on; add --yes to send the details");
                }
            }
        };
        if matches!(answer.as_str(), "y" | "Y" | "yes" | "YES" | "Yes") {
            draft = match read_one(&preview, "draftId", true) {
                Ok(d) => d,
                Err(e) => return raised(&e),
            };
        } else {
            out::stderr_line(&format!(
                "Sending your feedback without the {PRODUCT} details."
            ));
        }
    }
    let locale = ["LC_ALL", "LANG"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    let locale = locale.split('.').next().unwrap_or("").to_string();
    let mut b = vec![
        (json::Text::from("text"), env_text(&r.text)),
        (json::Text::from("client"), p::text("cli")),
    ];
    for (k, v) in [
        ("category", &r.category),
        ("scope", &r.scope),
        ("command", &r.command),
        ("locale", &locale),
    ] {
        if !v.is_empty() {
            b.push((json::Text::from(k), p::text(v)));
        }
    }
    if !draft.is_empty() {
        b.push((
            json::Text::from("diagnostics"),
            Value::Object(vec![(json::Text::from("draftId"), p::text(&draft))]),
        ));
    }
    let payload =
        json::to_string(&Value::Object(b), &Options::default()).expect("json.dumps of strings");
    let answer = match sf_call(ctx, true, "/cli/product-feedback/send", &payload) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let id = match read_one(&answer, "id", true) {
        Ok(id) => id,
        Err(e) => return raised(&e),
    };
    if id.is_empty() {
        out::stderr_line(r#"{"error":"daemon_error","hint":"the daemon answered without an id"}"#);
        return Exit::Refused;
    }
    let query = format!("id={}", quote(&id));
    let mut shown = Vec::new();
    for _ in 0..10 {
        shown = match sf_call(ctx, false, "/cli/product-feedback/show", &query) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let st = match state(&shown) {
            Ok(s) => s,
            Err(e) => return raised(&e),
        };
        if !matches!(st.as_str(), "queued" | "sending") {
            break;
        }
        if st == "queued" && item_error(&shown) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if r.json {
        return item_json(&shown).print();
    }
    result(&shown).print()
}

/// `[ -t 0 ] && [ -t 2 ] && { : < /dev/tty; }`, then the question on the
/// terminal and `IFS= read -r answer < /dev/tty || answer=""`. `None`:
/// there is no terminal to ask on.
fn ask_tty(question: &str) -> Option<String> {
    use crate::prompt;
    use std::io::Write;
    if !(prompt::stdin_is_tty() && prompt::stderr_is_tty() && prompt::dev_tty_readable()) {
        return None;
    }
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(question.as_bytes());
        let _ = tty.flush();
    }
    let line = prompt::read_dev_tty_line().ok()?;
    Some(String::from_utf8_lossy(line.expose()).into_owned())
}

/// `print(json.load(sys.stdin)["item"]["state"])` held in `$(…)`.
fn state(body: &[u8]) -> Result<String, Raise> {
    let v = p::load(body, 1)?;
    let it = p::idx(&v, "item", 1)?;
    Ok(cp::held(&p::idx(&it, "state", 1)?))
}

/// `sys.exit(0 if json.load(sys.stdin)["item"].get("error") else 1)`.
fn item_error(body: &[u8]) -> bool {
    p::load(body, 1)
        .and_then(|v| p::idx(&v, "item", 1))
        .and_then(|it| p::dget(&it, "error", 1))
        .is_ok_and(|e| p::truthy(&e))
}

/// `print(json.dumps(json.load(sys.stdin)["item"]))`.
fn item_json(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 1)?;
        let it = p::idx(&v, "item", 1)?;
        let text = json::to_string(&it, &Options::default())
            .map_err(|e| Raise::at(1, format!("ValueError: {e}")))?;
        o.line(&text);
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// What a report came to.
pub fn result(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let it = p::idx(&v, "item", 3)?;
        let st = p::idx(&it, "state", 4)?;
        if st == p::text("sent") {
            let id = p::idx(&it, "id", 6)?;
            let cat = p::idx(&it, "category", 6)?;
            o.line(&format!(
                "Sent to Alakazam Labs: {} ({}).",
                py::str_of(&id),
                py::str_of(&cat)
            ));
        } else if st == p::text("refused") {
            let id = p::idx(&it, "id", 8)?;
            let mut why = p::dget(&it, "errorMessage", 8)?;
            if !p::truthy(&why) {
                why = p::or(p::dget(&it, "error", 8)?, p::text(""));
            }
            o.line(&format!(
                "Refused by the feedback service: {} ({}). It will not be retried.",
                py::str_of(&id),
                py::str_of(&why)
            ));
        } else {
            let id = p::idx(&it, "id", 10)?;
            let cat = p::idx(&it, "category", 10)?;
            o.line(&format!(
                "Saved on this server: {} ({}). It sends when it can.",
                py::str_of(&id),
                py::str_of(&cat)
            ));
            if p::truthy(&p::dget(&it, "errorMessage", 11)?) {
                let m = p::idx(&it, "errorMessage", 12)?;
                o.line(&p::concat("  ", &m, 12)?);
            }
            let id = p::idx(&it, "id", 13)?;
            o.line(&format!(
                concat!("  Check it with: ", cli_name!(), " send-feedback show {}"),
                py::str_of(&id)
            ));
        }
        if p::truthy(&p::dget(&it, "hasDiagnostics", 14)?) {
            o.line(&format!(
                "  The {PRODUCT} details you approved are included."
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `_sf_print_sections`.
pub fn sections(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let mut secs = p::dget(&v, "sections", 4)?;
        if !p::truthy(&secs) {
            let item = p::or(p::dget(&v, "item", 4)?, Value::Object(vec![]));
            secs = p::or(p::dget(&item, "diagnostics", 4)?, p::empty_list());
        }
        for s in p::iter(&secs, 5)? {
            let title = p::dget_or(&s, "title", p::text(""), 6)?;
            let id = p::dget_or(&s, "id", p::text(""), 6)?;
            o.line(&format!(
                "\u{2500}\u{2500} {} ({}) \u{2500}\u{2500}",
                py::str_of(&title),
                py::str_of(&id)
            ));
            o.line(&py::str_of(&p::dget_or(&s, "text", p::text(""), 7)?));
            o.line("");
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn list(ctx: &mut Ctx, rest: &[&str]) -> Exit {
    let (mut state, mut json_out) = (String::new(), false);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--state" => {
                let Some(s) = rest.get(i + 1) else {
                    return usage("usage: --state <state>");
                };
                state = (*s).to_string();
                i += 2;
            }
            "--json" => {
                json_out = true;
                i += 1;
            }
            _ => return usage(LIST_USAGE),
        }
    }
    let q = if state.is_empty() {
        String::new()
    } else {
        format!("state={}", quote(&state))
    };
    let body = match sf_call(ctx, false, "/cli/product-feedback/list", &q) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if json_out {
        return body_line(&body);
    }
    list_reader(&body).print()
}

/// `" ".join(s.split())`.
fn squeeze(s: &str) -> String {
    s.split(py_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `v / 1000`, as Python's error says it for an `int` divisor.
fn per_1000(v: &Value, line: u32) -> Result<f64, Raise> {
    p::div(v, 1000.0, line).map_err(|mut e| {
        e.exception = e.exception.replace("and 'float'", "and 'int'");
        e
    })
}

/// The `list` reader.
pub fn list_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let items = p::or(p::dget(&v, "items", 4)?, p::empty_list());
        if !p::truthy(&items) {
            let all = p::dget(&v, "all", 6)?;
            o.line(&if p::truthy(&all) {
                format!("No feedback to {PRODUCT} on this server yet.")
            } else {
                format!("No feedback to {PRODUCT} sent from here yet.")
            });
            return Ok(Exit::Ok);
        }
        for it in p::iter(&items, 8)? {
            let at = per_1000(&p::idx(&it, "createdAt", 9)?, 9)?;
            let when = p::local_time(&Value::Float(at), "%Y-%m-%d %H:%M", 9)?;
            let text = p::dget_or(&it, "text", p::text(""), 10)?;
            if !matches!(text, Value::Str(_)) {
                return Err(Raise::no_attribute(10, &text, "split"));
            }
            let mut text = squeeze(&py::str_of(&text));
            if text.chars().count() > 60 {
                text = text.chars().take(57).collect::<String>() + "...";
            }
            let who = if p::truthy(&p::dget(&it, "mine", 13)?) {
                String::new()
            } else {
                p::concat("  [", &p::dget_or(&it, "sender", p::text(""), 13)?, 13)? + "]"
            };
            let scope = if p::truthy(&p::dget(&it, "scope", 14)?) {
                format!("  (scope {})", py::str_of(&p::idx(&it, "scope", 14)?))
            } else {
                String::new()
            };
            let id = p::idx(&it, "id", 15)?;
            let st = p::idx(&it, "state", 15)?;
            let cat = p::idx(&it, "category", 15)?;
            o.line(&format!(
                "{}  {} {} {when}  {text}{who}{scope}",
                py::str_of(&id),
                p::left(&st, 9),
                p::left(&cat, 6),
            ));
            if p::truthy(&p::dget(&it, "errorMessage", 16)?) {
                let st = p::idx(&it, "state", 16)?;
                if ["queued", "failed", "refused"]
                    .iter()
                    .any(|s| st == p::text(s))
                {
                    let m = p::idx(&it, "errorMessage", 17)?;
                    o.line(&p::concat("    ", &m, 17)?);
                }
            }
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn show_one(ctx: &mut Ctx, rest: &[&str]) -> Exit {
    let (mut id, mut json_out) = (String::new(), false);
    for w in rest {
        match *w {
            "--json" => json_out = true,
            w if w.starts_with('-') => return usage(SHOW_USAGE),
            w => {
                if !id.is_empty() {
                    return usage(SHOW_USAGE);
                }
                id = w.to_string();
            }
        }
    }
    if id.is_empty() {
        return usage(SHOW_USAGE);
    }
    let body = match sf_call(
        ctx,
        false,
        "/cli/product-feedback/show",
        &format!("id={}", quote(&id)),
    ) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if json_out {
        return body_line(&body);
    }
    show_reader(&body).print()
}

/// `t(ms)` in the `show` reader, called from the module's line `from`.
fn t(ms: &Value, from: u32) -> Result<String, Raise> {
    if !p::truthy(ms) {
        return Ok("-".into());
    }
    let inside = |e: Raise| Raise {
        frames: vec![(from, "<module>"), (5, "t")],
        exception: e.exception,
    };
    let at = per_1000(ms, 5).map_err(inside)?;
    p::local_time(&Value::Float(at), "%Y-%m-%d %H:%M:%S", 5).map_err(inside)
}

/// The `show` reader.
pub fn show_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let it = p::idx(&v, "item", 3)?;
        let (id, st, cat) = (
            p::idx(&it, "id", 6)?,
            p::idx(&it, "state", 6)?,
            p::idx(&it, "category", 6)?,
        );
        o.line(&format!(
            "{}  {}  {}",
            py::str_of(&id),
            py::str_of(&st),
            py::str_of(&cat)
        ));
        let g = |k: &str, line| p::dget_or(&it, k, p::text(""), line).map(|v| py::str_of(&v));
        o.line(&format!(
            "  from:     {} ({}, {})",
            g("sender", 7)?,
            g("senderKind", 7)?,
            g("client", 7)?
        ));
        if p::truthy(&p::dget(&it, "scope", 8)?) {
            o.line(&format!(
                "  scope:    {}",
                py::str_of(&p::idx(&it, "scope", 9)?)
            ));
        }
        o.line(&format!(
            "  created:  {}",
            t(&p::dget(&it, "createdAt", 10)?, 10)?
        ));
        if p::truthy(&p::dget(&it, "sentAt", 11)?) {
            let sent = t(&p::idx(&it, "sentAt", 12)?, 12)?;
            let remote = p::or(p::dget(&it, "remoteId", 12)?, p::text("-"));
            o.line(&format!(
                "  sent:     {sent} (Alakazam id {})",
                py::str_of(&remote)
            ));
        } else if p::idx(&it, "state", 13)? == p::text("queued") {
            let next = t(&p::dget(&it, "nextAttemptAt", 14)?, 14)?;
            let tries = p::dget_or(&it, "attempts", Value::Int("0".into()), 14)?;
            o.line(&format!(
                "  next try: {next} (attempts so far: {})",
                py::str_of(&tries)
            ));
        }
        if p::truthy(&p::dget(&it, "errorMessage", 15)?) {
            o.line(&format!(
                "  reason:   {}",
                py::str_of(&p::idx(&it, "errorMessage", 16)?)
            ));
        }
        if p::truthy(&p::dget(&it, "contactEmail", 17)?) {
            o.line(&format!(
                "  contact:  {}",
                py::str_of(&p::idx(&it, "contactEmail", 18)?)
            ));
        }
        o.line("");
        o.line(&g("text", 20)?);
        let d = p::dget(&it, "diagnostics", 21)?;
        if p::truthy(&d) {
            o.line("");
            o.line(&format!("{PRODUCT} details included:"));
            for s in p::iter(&d, 25)? {
                let title = p::dget_or(&s, "title", p::text(""), 26)?;
                o.line(&format!(
                    "\u{2500}\u{2500} {} \u{2500}\u{2500}",
                    py::str_of(&title)
                ));
                o.line(&py::str_of(&p::dget_or(&s, "text", p::text(""), 27)?));
            }
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn transition(ctx: &mut Ctx, verb: &str, rest: &[&str]) -> Exit {
    let id = match rest {
        [id] if !id.is_empty() => *id,
        _ => {
            return usage(&format!(
                concat!("usage: ", cli_name!(), " send-feedback {} <id>"),
                verb
            ))
        }
    };
    let body = Cj::new().s("id", id).body();
    if let Err(e) = sf_call(ctx, true, &format!("/cli/product-feedback/{verb}"), &body) {
        return e;
    }
    stdout_line(&if verb == "retry" {
        format!(
            concat!(
                "Queued again: {}. It sends when it can (",
                cli_name!(),
                " send-feedback show {})."
            ),
            id, id
        )
    } else {
        format!("Cancelled: {id}. It will not be sent.")
    })
}

/// `send-feedback settings`'s words, read in the shell's order: the first
/// mistake is the line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SettingsAsk {
    pub enabled: Option<bool>,
    /// `off`, `on`, `cli` or `all`.
    pub agents: Option<&'static str>,
    pub attach: Option<String>,
    pub agent_diag: Option<bool>,
    pub json: bool,
}

/// `_sf_onoff`.
fn onoff(word: &str, flag: &str) -> Result<bool, String> {
    match word {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(format!("usage: {flag} takes on or off")),
    }
}

/// `_sf_settings_run settings`'s loop and checks.
pub fn parse_settings(words: &[&str]) -> Result<SettingsAsk, String> {
    parse_settings_as(words, SETTINGS_USAGE, true)
}

/// `_sf_settings_run <who>`'s loop and checks: `usage` is the who's usage
/// line, and `setup` (`setup agent-reports`, `settings` false) takes
/// neither `--enabled` nor `--agent-diagnostics` (its usage line, before
/// the value is read).
pub fn parse_settings_as(
    words: &[&str],
    usage: &str,
    settings: bool,
) -> Result<SettingsAsk, String> {
    let mut a = SettingsAsk::default();
    let mut i = 0;
    while i < words.len() {
        let next = words.get(i + 1).copied();
        match words[i] {
            flag @ ("--enabled" | "--agent-diagnostics") => {
                if !settings {
                    return Err(usage.into());
                }
                let v = next.ok_or_else(|| format!("usage: {flag} on|off"))?;
                let on = onoff(v, flag)?;
                if flag == "--enabled" {
                    a.enabled = Some(on);
                } else {
                    a.agent_diag = Some(on);
                }
                i += 2;
            }
            "--agents" => {
                let v = next.ok_or("usage: --agents off|cli|all")?;
                a.agents = Some(match v {
                    "off" | "false" | "no" | "0" => "off",
                    "on" | "true" | "yes" | "1" => "on",
                    "cli" => "cli",
                    "all" => "all",
                    _ => return Err("usage: --agents takes off, cli or all".into()),
                });
                i += 2;
            }
            "--attach" => {
                match next {
                    Some(v) if !blank(v.as_bytes()) => a.attach = Some(v.to_string()),
                    _ => {
                        return Err(concat!(
                            "usage: --attach <parts joined by commas>|none|all (",
                            cli_name!(),
                            " send-feedback settings lists the parts)"
                        )
                        .into())
                    }
                }
                i += 2;
            }
            "--json" => {
                a.json = true;
                i += 1;
            }
            _ => return Err(usage.into()),
        }
    }
    if a.attach.is_some() && a.agent_diag.is_some() {
        return Err(
            "usage: --attach (the parts) or --agent-diagnostics (all or none), not both".into(),
        );
    }
    Ok(a)
}

fn settings(ctx: &mut Ctx, rest: &[&str]) -> Exit {
    let a = match parse_settings(rest) {
        Ok(a) => a,
        Err(line) => return usage(&line),
    };
    let body = match settings_calls(&a, &mut |post, payload| {
        sf_call(ctx, post, SETTINGS, payload)
    }) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if a.json {
        return body_line(&body);
    }
    let text = render(&super::without_nuls(&body));
    match out::write_flushed(&mut std::io::stdout().lock(), &text) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// One `_sf_settings_call`: POST or not, the payload; the body on a 200,
/// else its end (its lines printed).
pub type SettingsCall<'a> = dyn FnMut(bool, &str) -> Result<Vec<u8>, Exit> + 'a;

/// `_sf_settings_run`'s calls for `a`, each one `call(post, payload)` on
/// the settings route (`_sf_settings_call`: an `Err` is its end, its
/// lines printed): a read; or a write, after a read that tells an older
/// server apart when the words need a 0.46.17 one (that older server is
/// its line, exit 1, nothing changed). The last call's body.
pub fn settings_calls(a: &SettingsAsk, call: &mut SettingsCall<'_>) -> Result<Vec<u8>, Exit> {
    let writes =
        a.enabled.is_some() || a.agents.is_some() || a.agent_diag.is_some() || a.attach.is_some();
    if !writes {
        return call(false, "");
    }
    let mut new = true;
    if a.attach.is_some() || matches!(a.agents, Some("on" | "cli" | "all")) {
        let got = call(false, "")?;
        new = contains(&got, b"\"agentsAsked\"") || contains(&got, b"\"attachParts\"");
    }
    if !new && (a.attach.is_some() || matches!(a.agents, Some("cli" | "all"))) {
        out::stderr_line(&format!(
            concat!(
                "This server's {} is older: --agents cli|all and --attach need its update (",
                cli_name!(),
                " update). Nothing was changed."
            ),
            PRODUCT
        ));
        return Err(Exit::Error);
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(on) = a.enabled {
        parts.push(format!("\"enabled\": {on}"));
    }
    match a.agents {
        Some("off") => parts.push("\"agents\": false".into()),
        Some("on") if !new => parts.push("\"agents\": true".into()),
        Some("on" | "cli") => parts.push("\"agents\": true, \"agentPlatform\": false".into()),
        Some("all") => parts.push("\"agents\": true, \"agentPlatform\": true".into()),
        _ => {}
    }
    if let Some(at) = &a.attach {
        parts.push(format!("\"agentAttach\": {}", super::setup::jstr(at)));
    }
    if let Some(on) = a.agent_diag {
        parts.push(format!("\"agentDiagnostics\": {on}"));
    }
    let payload = format!("{{{}}}", parts.join(", "));
    call(true, &payload)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `[[:space:]]` in the C locale.
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// For each `"<key>"` in `t` followed by `[[:space:]]*:[[:space:]]*`:
/// where that `"` is, and where the run after the `:` ends.
fn keys(t: &[u8], key: &str) -> Vec<(usize, usize)> {
    let quoted = format!("\"{key}\"");
    let q = quoted.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = t[from..].windows(q.len()).position(|w| w == q) {
        let start = from + at;
        from = start + 1;
        let mut i = start + q.len();
        while i < t.len() && space(t[i]) {
            i += 1;
        }
        if t.get(i) != Some(&b':') {
            continue;
        }
        i += 1;
        while i < t.len() && space(t[i]) {
            i += 1;
        }
        out.push((start, i));
    }
    out
}

/// `_sf_jbool`: `true`, `false`, or nothing.
fn jbool(key: &str, t: &[u8]) -> &'static str {
    for (_, i) in keys(t, key) {
        if t[i..].starts_with(b"true") {
            return "true";
        }
        if t[i..].starts_with(b"false") {
            return "false";
        }
    }
    ""
}

/// `_sf_jnum`: the digits, or nothing.
fn jnum(key: &str, t: &[u8]) -> String {
    for (_, i) in keys(t, key) {
        let n = t[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if n > 0 {
            return String::from_utf8_lossy(&t[i..i + n]).into_owned();
        }
    }
    String::new()
}

/// `_sf_jarr`: what is between the array's brackets, or nothing.
fn jarr(key: &str, t: &[u8]) -> Vec<u8> {
    for (_, i) in keys(t, key) {
        if t.get(i) == Some(&b'[') {
            if let Some(end) = t[i + 1..].iter().position(|b| *b == b']') {
                return t[i + 1..i + 1 + end].to_vec();
            }
        }
    }
    Vec::new()
}

/// `_setup_jfield KEY TEXT`, when it found one.
fn jfield(key: &str, t: &[u8]) -> Option<Vec<u8>> {
    super::setup::jfield(key, t)
}

/// `_sf_oo`.
fn oo(v: &str) -> &'static str {
    if v == "true" {
        "on"
    } else {
        "off"
    }
}

/// `printf '%-<n>s'` in the C locale: padded by bytes.
fn pad(s: &[u8], n: usize) -> Vec<u8> {
    let mut v = s.to_vec();
    while v.len() < n {
        v.push(b' ');
    }
    v
}

/// `_sf_settings_render` on the answer (NULs gone).
pub fn render(body: &[u8]) -> Vec<u8> {
    let mut v = body.to_vec();
    let mut svc_msg = Vec::new();
    for (start, i) in keys(&v, "service") {
        if v.get(i) != Some(&b'{') {
            continue;
        }
        if let Some(end) = v[i..].iter().position(|b| *b == b'}') {
            let svc = v[start..i + end + 1].to_vec();
            v.drain(start..i + end + 1);
            if let Some(m) = jfield("message", &svc) {
                svc_msg = m;
            }
            break;
        }
    }
    let mut o: Vec<u8> = Vec::new();
    let mut line = |parts: &[&[u8]]| {
        for p in parts {
            o.extend_from_slice(p);
        }
        o.push(b'\n');
    };
    line(&[format!("Send feedback to {PRODUCT} on this server").as_bytes()]);
    line(&[
        b"  enabled:            ",
        oo(jbool("enabled", &v)).as_bytes(),
    ]);
    if contains(&v, b"\"agentsAsked\"") || contains(&v, b"\"attachParts\"") {
        let agents = jbool("agents", &v);
        let platform = jbool("agentPlatform", &v);
        let what = if agents != "true" {
            "nothing (off)".to_string()
        } else if platform == "true" {
            format!("anything about {PRODUCT}")
        } else {
            format!("problems with {PRODUCT} commands")
        };
        line(&[b"  agents report:      ", what.as_bytes()]);
        if jbool("agentsAsked", &v) == "true" {
            let mut by = b"-".to_vec();
            if let Some(b) = jfield("agentsAskedBy", &v).filter(|b| !b.is_empty()) {
                by = b;
            }
            if by == b"owner" {
                by = b"the owner".to_vec();
            }
            let at = jnum("agentsAskedAt", &v);
            let when = if at.is_empty() {
                "-".to_string()
            } else {
                let secs = at
                    .trim_start_matches('0')
                    .parse::<i128>()
                    .map(|n| n / 1000)
                    .unwrap_or(0);
                p::local_time(&Value::Int(secs.to_string()), "%Y-%m-%d %H:%M", 1)
                    .unwrap_or_else(|_| "-".to_string())
            };
            line(&[b"  answered:           by ", &by, b", ", when.as_bytes()]);
        } else {
            line(&[b"  answered:           not asked yet"]);
        }
        let allowed = jarr("agentAttach", &v);
        line(&[b"  agents may attach:  (the versions always go)"]);
        let parts = jarr("attachParts", &v);
        let mut rest: &[u8] = &parts;
        while let Some(open) = rest.iter().position(|b| *b == b'{') {
            let Some(close) = rest[open..].iter().position(|b| *b == b'}') else {
                break;
            };
            let obj = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            let Some(id) = jfield("id", obj) else {
                continue;
            };
            let label = jfield("label", obj).unwrap_or_default();
            let quoted = [b"\"".as_slice(), &id, b"\""].concat();
            let state: &[u8] = if contains(&allowed, &quoted) {
                b"on"
            } else {
                b"off"
            };
            line(&[b"    ", &pad(state, 3), b"  ", &pad(&id, 13), b"  ", &label]);
        }
    } else {
        line(&[
            b"  agents:             ",
            oo(jbool("agents", &v)).as_bytes(),
        ]);
        line(&[
            b"  agent diagnostics:  ",
            oo(jbool("agentDiagnostics", &v)).as_bytes(),
        ]);
    }
    if jbool("canSend", &v) == "true" {
        line(&[b"  can send now:       yes"]);
    } else {
        line(&[b"  can send now:       no"]);
    }
    if let Some(m) = jfield("message", &v).filter(|m| !m.is_empty()) {
        line(&[b"    ", &m]);
    }
    if !svc_msg.is_empty() {
        line(&[b"  service:            ", &svc_msg]);
    }
    let pending = jnum("pending", &v);
    if !pending.is_empty() && pending != "0" {
        line(&[b"  waiting to send:    ", pending.as_bytes()]);
    }
    let endpoint = jfield("endpoint", &v).unwrap_or_default();
    line(&[b"  sends to:           ", &endpoint]);
    o
}

#[cfg(test)]
mod tests;
