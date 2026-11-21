//! `explain [<id>] [--json]`: what an App refusal id means, as the shell
//! CLI's `cmd_explain` prints it (`cli/src/explain.sh`), from the refusal
//! registry built into the CLI ([`registry`]: `_k2_refusals_json`, which
//! contract-gen writes into `cli/src/study-app-api.sh`; the binary embeds
//! the same file). Nothing is asked of the daemon.
//!
//! Before it, the shell's prelude ([`crate::offline`]): a setting bash
//! reads at start, a missing program and the like hand the command to the
//! shell. The connection gate is skipped. The virtual desktop's guard is
//! not, unless a word is `-h` or `--help`: with `NO_OWNER_FALLBACK=1` and
//! no token it is the guard's three lines and exit 3. The tool is no
//! governed one: no session check runs.
//!
//! The words, in order: `--json`; `-h` or `--help` prints the usage
//! (`cli/text/help/explain.txt`), exit 0; any other word starting with
//! `-` is `Unknown flag: <word>` and the usage line, exit 2; the first
//! other word is the id (an empty one leaves it empty), and a second
//! non-empty one is the usage line, exit 2.
//!
//! The id: stripped and lowercased, as Python's `str.strip().lower()`.
//! None: every id by area ([`list`]), or with `--json` the registry. Else
//! its row by id, then by alias ([`row_text`], or the row as JSON), or the
//! unknown-id answer, exit 4.
//!
//! A Markdown kit id (`MPC-`, `MPCE-`, any case) is the kit's own
//! explain, as the shell's `_explain_kit` asks it ([`kit`]): one
//! `_compute_send POST /cli/board/file/verb {"argv": ["explain", <id>],
//! "json": <bool>}`, the route that answers the kit's pages with no file,
//! and no path in it, so no file is read. Before 0.46.17 both sides sent
//! it to `cmd_doc explain`, which took the id as a doc's path. The shell
//! keeps ([`run`] gives `None`): a word that is not UTF-8, and a kit id
//! with no port curl takes, no token, or `HOOK_SOCK` with a `CODEX_`
//! variable. So does a line with `-h` or `--help` in it: the
//! front prints `explain --help` and `explain -h` as help pages and hands
//! any other such line to the shell ([`crate::front`]); [`page`] still
//! reads the words as `cmd_explain` does.
//!
//! The paragraphs are wrapped as Python's `textwrap.fill` wraps them
//! ([`wrap`]). Python's `\w` and `\d` are Unicode classes; here a word
//! character is `char::is_alphanumeric` or `_` and a digit
//! `char::is_numeric` (ASCII: `0`-`9`), which differ only for a few
//! non-ASCII numerals. `tests/explain_parity.rs` checks every row of the
//! registry against the shell.

use super::help::{shell_prelude, show};
use crate::exit::Exit;
use crate::offline::{Env, Prelude};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::pages::Page;
use crate::prelude::VIRTUAL_DESKTOP;
use crate::text;
use k2_client::env_names;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The registry `_k2_refusals_json` prints, without its last newline
/// (the App family reads the same heredoc: [`super::app::call`]).
pub fn registry() -> &'static str {
    super::app::call::registry_json()
}

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["explain"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 explain ...`: the page, or `None` when the shell answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let (env, p) = shell_prelude()?;
    if let Some(words) = kit_call(args, &p, &env) {
        return kit(args, &words[1], words.len() > 2);
    }
    Some(show(&page(args, &p, &env)?))
}

/// The words for a Markdown kit id (`explain <id> [--json]`), past the
/// virtual desktop's guard; `None` for any other line.
fn kit_call(args: &[OsString], p: &Prelude, env: &Env) -> Option<Vec<String>> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let no_owner = env
        .first(env_names::NO_OWNER_FALLBACK)
        .is_some_and(|v| v == "1");
    if no_owner && !p.token {
        return None;
    }
    match parse(&words) {
        Ask::Look { id, json } if kit_id(&id) => {
            let mut w = vec!["explain".to_string(), id];
            if json {
                w.push("--json".to_string());
            }
            Some(w)
        }
        _ => None,
    }
}

/// The route that answers the kit's own pages with no file.
pub const KIT_ROUTE: &str = "/cli/board/file/verb";

/// `_explain_kit <id> <json>`: the kit's explain for `id` on [`KIT_ROUTE`],
/// then the shell's reader ([`kit_answer`]) and the EXIT trap. `None`:
/// the shell's (no port curl takes, no token, or a Codex session).
fn kit(args: &[OsString], id: &str, as_json: bool) -> Option<Exit> {
    if super::hook_sock().is_some()
        && crate::prelude::codex_env(std::env::vars_os().map(|(n, _)| n))
    {
        return None;
    }
    let mut settings = crate::daemon::current().ok()?;
    settings.port = settings.port_text.as_deref().and_then(super::curl_port);
    if settings.port.is_none() || settings.auth == crate::transport::Auth::None {
        return None;
    }
    let would_refuse = crate::passport::current(args).would_refuse();
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            crate::out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let words: Vec<&str> = args.iter().skip(1).filter_map(|a| a.to_str()).collect();
    let mut ctx = super::app::call::Ctx::new(client, super::app::call::args_json(&words));
    let exit = match ctx.send(true, KIT_ROUTE, &kit_body(id, as_json), false) {
        Ok(split) => kit_answer(&split.status, &split.body, as_json).print(),
        Err(exit) => exit,
    };
    let super::app::call::Ctx {
        mut client,
        closing,
        ..
    } = ctx;
    closing.flush(&mut client, exit);
    Some(exit)
}

/// The request's body, as `_explain_kit` builds it (`_setup_jstr`).
pub fn kit_body(id: &str, as_json: bool) -> String {
    format!(
        "{{\"argv\":[\"explain\",{}],\"json\":{as_json}}}",
        super::setup::jstr(id)
    )
}

/// `sys.exit(x)`: an int its low byte (`True` 1), `None`, `False` and a
/// missing one 0; anything else printed on stderr, then 1.
fn py_exit(x: Option<&Value>, stderr: &mut String) -> Exit {
    match x {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Exit::Ok,
        Some(Value::Bool(true)) => Exit::Error,
        Some(Value::Int(d)) => match d.parse::<i64>() {
            Ok(n) => Exit::from_curl(i32::from(n.rem_euclid(256) as u8)),
            Err(_) => Exit::Error,
        },
        Some(other) => {
            stderr.push_str(&py::str_of(other));
            stderr.push('\n');
            Exit::Error
        }
    }
}

/// `_explain_kit`'s reader on `_compute_split`'s status and body.
pub fn kit_answer(status: &str, body: &[u8], as_json: bool) -> super::Shown {
    let raw = String::from_utf8_lossy(body);
    let stripped = raw.trim_matches(super::py_space);
    let v = if stripped.is_empty() {
        Value::Object(Vec::new())
    } else {
        crate::out::pylib::json::loads(&raw).unwrap_or(Value::Object(Vec::new()))
    };
    let v = match v {
        Value::Object(_) => v,
        _ => Value::Object(Vec::new()),
    };
    let field = |k: &str| key(&v, k).cloned();
    let (mut stdout, mut stderr) = (String::new(), String::new());
    if status == "200" {
        if let Some(Value::Str(t)) = field("stdout") {
            stdout.push_str(&py::str_of(&Value::Str(t)));
            let x = field("exit").filter(py::truthy);
            let exit = py_exit(x.as_ref(), &mut stderr);
            return super::Shown {
                stdout,
                stderr,
                exit,
            };
        }
    }
    let shown = |stdout, stderr, exit| super::Shown {
        stdout,
        stderr,
        exit,
    };
    let s_of = |x: Option<Value>| x.map_or_else(|| "None".to_string(), |x| py::str_of(&x));
    if field("id").is_some() && field("exit").is_some() {
        if as_json {
            stdout = format!("{stripped}\n");
        } else {
            stderr = format!(
                "Error [{}] {}: {}\n",
                s_of(field("code")),
                s_of(field("id")),
                s_of(field("message"))
            );
            if let Some(next) = field("next").filter(py::truthy) {
                stderr.push_str(&format!("Next: {}\n", py::str_of(&next)));
            }
        }
        let exit = match field("exit") {
            Some(x @ (Value::Int(_) | Value::Bool(_))) => py_exit(Some(&x), &mut stderr),
            _ => Exit::Error,
        };
        return shown(stdout, stderr, exit);
    }
    let err = match field("error") {
        Some(e @ Value::Object(_)) => e,
        _ => Value::Object(Vec::new()),
    };
    let given = |k: &str| key(&err, k).filter(|x| py::truthy(x)).cloned();
    let code = given("code").unwrap_or_else(|| {
        Value::Str(Text::from(if status == "000" {
            "daemon_unreachable"
        } else {
            "error"
        }))
    });
    let hint = given("hint").unwrap_or_else(|| {
        Value::Str(Text::from(if status == "000" {
            concat!("no answer from the ", product_name!(), " daemon").to_string()
        } else {
            format!("the daemon refused (HTTP {status})")
        }))
    });
    if as_json {
        if stripped.is_empty() {
            let e = Value::Object(vec![(
                Text::from("error"),
                Value::Object(vec![
                    (Text::from("code"), code.clone()),
                    (Text::from("hint"), hint.clone()),
                ]),
            )]);
            stdout = format!("{}\n", dumps(&e));
        } else {
            stdout = format!("{stripped}\n");
        }
    } else {
        stderr = format!("Error [{}]: {}\n", py::str_of(&code), py::str_of(&hint));
    }
    let refused = ["not_enabled", "forbidden", "not_connected", "sandboxed_agent", "core_off"]
        .iter()
        .any(|c| matches!(&code, Value::Str(t) if t.as_str() == Some(*c)));
    shown(stdout, stderr, if refused { Exit::Refused } else { Exit::Error })
}

/// The usage line of a usage error.
pub const USAGE: &str = concat!("usage: ", cli_name!(), " explain [<id>] [--json]");
/// The list's first line.
pub const LIST_HEAD: &str = concat!(
    "Refusal ids by area. ",
    cli_name!(),
    " explain <id> prints what happened and the fix."
);
/// A row that can be asked for.
pub const ASKABLE: &str = concat!(
    "can be asked for: yes (the error ends with Ask with: ",
    cli_name!(),
    " permission request \u{2026}; run it as printed)"
);
/// The unknown id's hint, in its JSON.
pub const UNKNOWN_HINT: &str = concat!(
    "no App refusal by that id: ",
    cli_name!(),
    " explain lists them"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ask {
    Usage,
    Bad(String),
    Look { id: String, json: bool },
}

fn parse(words: &[&str]) -> Ask {
    let (mut id, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            "-h" | "--help" => return Ask::Usage,
            w if w.starts_with('-') => {
                return Ask::Bad(format!("Unknown flag: {w}\n{USAGE}\n"));
            }
            w => {
                if !id.is_empty() {
                    return Ask::Bad(format!("{USAGE}\n"));
                }
                id = w.to_string();
            }
        }
    }
    Ask::Look { id, json }
}

/// Whether `id`, uppercased, starts as a Markdown kit id.
fn kit_id(id: &str) -> bool {
    let up = id.to_ascii_uppercase();
    up.starts_with("MPC-") || up.starts_with("MPCE-")
}

/// Python's `str.strip()`.
fn py_strip(s: &str) -> &str {
    s.trim_matches(super::py_space)
}

/// The page for `args` (the first word `explain`), with the shell's
/// prelude `p` in `env`, or `None` when the shell answers.
pub fn page(args: &[OsString], p: &Prelude, env: &Env) -> Option<Page> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let no_owner = env
        .first(env_names::NO_OWNER_FALLBACK)
        .is_some_and(|v| v == "1");
    let help = words.iter().any(|w| *w == "-h" || *w == "--help");
    if no_owner && !p.token && !help {
        let text: String = VIRTUAL_DESKTOP.iter().map(|l| format!("{l}\n")).collect();
        return Some(lines(&text, true, Exit::Refused));
    }
    match parse(&words) {
        Ask::Usage => Some(lines(text::help("explain")?, false, Exit::Ok)),
        Ask::Bad(text) => Some(lines(&text, true, Exit::Usage)),
        Ask::Look { id, json } => {
            if kit_id(&id) {
                return None;
            }
            let reg = json::parse(registry()).ok()?;
            Some(answer(&reg, &id, json))
        }
    }
}

fn lines(text: &str, stderr: bool, exit: Exit) -> Page {
    Page {
        text: text.to_string().into(),
        stderr,
        exit,
        lead: None,
    }
}

/// `d[key]` on an object the registry holds.
fn key<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    match v {
        Value::Object(pairs) => super::get(pairs, k),
        _ => None,
    }
}

fn items(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn s(v: Option<&Value>) -> String {
    v.map(py::str_of).unwrap_or_default()
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// The answer for the stripped and lowercased `id` in the registry `reg`.
fn answer(reg: &Value, id: &str, as_json: bool) -> Page {
    let want = py_strip(id).to_lowercase();
    let rows = items(key(reg, "rows"));
    if want.is_empty() {
        if as_json {
            let all = Value::Object(vec![
                (
                    Text::from("schema"),
                    key(reg, "schema").cloned().unwrap_or(Value::Null),
                ),
                (
                    Text::from("areas"),
                    key(reg, "areas").cloned().unwrap_or(Value::Null),
                ),
                (Text::from("rows"), Value::Array(rows.to_vec())),
            ]);
            return lines(&format!("{}\n", dumps(&all)), false, Exit::Ok);
        }
        return lines(&list(reg), false, Exit::Ok);
    }
    let is =
        |v: Option<&Value>| matches!(v, Some(Value::Str(t)) if t.as_str() == Some(want.as_str()));
    let found = rows.iter().find(|r| is(key(r, "id"))).or_else(|| {
        rows.iter()
            .find(|r| items(key(r, "aliases")).iter().any(|a| is(Some(a))))
    });
    let Some(row) = found else {
        if as_json {
            let err = Value::Object(vec![
                (Text::from("error"), Value::Str(Text::from("unknown_id"))),
                (Text::from("id"), Value::Str(Text::from(want.as_str()))),
                (Text::from("hint"), Value::Str(Text::from(UNKNOWN_HINT))),
            ]);
            return lines(&format!("{}\n", dumps(&err)), false, Exit::NotFound);
        }
        let line = format!(
            "{} explain: no App refusal id {} ({} explain lists them; {} study app-errors has all of them)\n",
            cli_name!(),
            py::repr(&Value::Str(Text::from(want.as_str()))),
            cli_name!(),
            cli_name!()
        );
        return lines(&line, true, Exit::NotFound);
    };
    if as_json {
        return lines(&format!("{}\n", dumps(row)), false, Exit::Ok);
    }
    lines(&row_text(row), false, Exit::Ok)
}

/// Every id by area.
fn list(reg: &Value) -> String {
    let rows = items(key(reg, "rows"));
    let mut out = format!("{LIST_HEAD}\n");
    for a in items(key(reg, "areas")) {
        let area = key(a, "area");
        let ids: Vec<String> = rows
            .iter()
            .filter(|r| key(r, "area") == area)
            .map(|r| {
                let retired = matches!(key(r, "status"), Some(Value::Str(t)) if t.as_str() == Some("retired"));
                format!("{}{}", s(key(r, "id")), if retired { " (retired)" } else { "" })
            })
            .collect();
        if ids.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(&format!("{} ({}):\n", s(area), s(key(a, "about"))));
        out.push_str(&fill(&ids.join(", "), 76, "  ", "  ", false));
        out.push('\n');
    }
    out
}

/// One row, as text.
fn row_text(row: &Value) -> String {
    let who_key = s(key(row, "who"));
    let who = match who_key.as_str() {
        "agent" => "you (the caller)".to_string(),
        "owner" => "the server's owner".to_string(),
        "admin" => "the owner or an Admin".to_string(),
        "person" => "the person signed in".to_string(),
        "k2" => concat!(product_name!(), " (wait, retry or update)").to_string(),
        other => other.to_string(),
    };
    let mut out = format!(
        "{}  ({} \u{b7} HTTP {} \u{b7} exit {})\n",
        s(key(row, "id")),
        s(key(row, "area")),
        s(key(row, "http")),
        s(key(row, "exit"))
    );
    if matches!(key(row, "status"), Some(Value::Str(t)) if t.as_str() == Some("retired")) {
        out.push_str(concat!(
            "retired: this ",
            product_name!(),
            " no longer sends it; older ",
            product_name!(),
            " servers may.\n"
        ));
    }
    out.push_str(&fill(&s(key(row, "what")), 76, "", "", true));
    out.push('\n');
    out.push_str(&fill(
        &format!("fix: {}", s(key(row, "fix"))),
        76,
        "",
        "     ",
        true,
    ));
    out.push('\n');
    out.push_str(&format!("who fixes it: {who}\n"));
    if key(row, "askable").is_some_and(py::truthy) {
        out.push_str(ASKABLE);
        out.push('\n');
    }
    let aliases = items(key(row, "aliases"));
    if !aliases.is_empty() {
        let names: Vec<String> = aliases.iter().map(py::str_of).collect();
        out.push_str(&format!("also sent as: {}\n", names.join(", ")));
    }
    out
}

/// `textwrap.fill(text, width=width, initial_indent=first,
/// subsequent_indent=rest, break_on_hyphens=hyphens)`.
pub fn fill(text: &str, width: usize, first: &str, rest: &str, hyphens: bool) -> String {
    wrap(text, width, first, rest, hyphens).join("\n")
}

/// The whitespace `textwrap` knows (`_whitespace`).
fn ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ')
}

/// Python's `\w`.
fn word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Python's `\d`.
fn digit(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        c.is_numeric()
    }
}

/// `textwrap`'s `letter`: `[^\d\W]`.
fn letter(c: char) -> bool {
    word(c) && !digit(c)
}

/// `textwrap`'s `word_punct`: `[\w!"'&.,?]`.
fn word_punct(c: char) -> bool {
    word(c) || "!\"'&.,?".contains(c)
}

/// `str.expandtabs()` (tab size 8), then every `textwrap` whitespace
/// character as a space.
fn munge(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut col = 0usize;
    for c in text.chars() {
        match c {
            '\t' => {
                let n = 8 - col % 8;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            '\n' | '\r' => {
                out.push(' ');
                col = 0;
            }
            c if ws(c) => {
                out.push(' ');
                col += 1;
            }
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// `TextWrapper._split`: `wordsep_re` (`hyphens`) or `wordsep_simple_re`.
fn chunks(t: &[char], hyphens: bool) -> Vec<Vec<char>> {
    let n = t.len();
    let at = |k: usize, back: usize| -> Option<char> {
        k.checked_sub(back).and_then(|i| t.get(i).copied())
    };
    let is = |c: Option<char>, f: fn(char) -> bool| c.is_some_and(f);
    let dashes = |from: usize| t[from..].iter().take_while(|c| **c == '-').count();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if ws(t[i]) {
            let j = i + t[i..].iter().take_while(|c| ws(**c)).count();
            out.push(t[i..j].to_vec());
            i = j;
            continue;
        }
        if !hyphens {
            let j = i + t[i..].iter().take_while(|c| !ws(**c)).count();
            out.push(t[i..j].to_vec());
            i = j;
            continue;
        }
        // An em-dash between words.
        if i > 0 && word_punct(t[i - 1]) {
            let d = dashes(i);
            if d >= 2 && is(t.get(i + d).copied(), word) {
                out.push(t[i..i + d].to_vec());
                i += d;
                continue;
            }
        }
        // A word, as short as it can be.
        let mut j = i + 1;
        let end = loop {
            // A hyphenated word: the hyphen ends it.
            if t.get(j) == Some(&'-') {
                let behind = (is(at(j, 2), letter) && is(at(j, 1), letter))
                    || (is(at(j, 3), letter) && at(j, 2) == Some('-') && is(at(j, 1), letter));
                let ahead = is(t.get(j + 1).copied(), letter)
                    && (is(t.get(j + 2).copied(), letter)
                        || (t.get(j + 2) == Some(&'-') && is(t.get(j + 3).copied(), letter)));
                if behind && ahead {
                    break j + 1;
                }
            }
            // The end of a word.
            if j == n || ws(t[j]) {
                break j;
            }
            // Before an em-dash.
            if word_punct(t[j - 1]) {
                let d = dashes(j);
                if d >= 2 && is(t.get(j + d).copied(), word) {
                    break j;
                }
            }
            j += 1;
        };
        out.push(t[i..end].to_vec());
        i = end;
    }
    out
}

/// `chunk.strip() == ''`.
fn blank(chunk: &[char]) -> bool {
    chunk.iter().all(|c| super::py_space(*c))
}

/// `TextWrapper.wrap` with `break_long_words` and `drop_whitespace` on.
pub fn wrap(text: &str, width: usize, first: &str, rest: &str, hyphens: bool) -> Vec<String> {
    let mut chunks = chunks(&munge(text), hyphens);
    chunks.reverse();
    let mut lines: Vec<String> = Vec::new();
    while !chunks.is_empty() {
        let mut cur: Vec<Vec<char>> = Vec::new();
        let mut cur_len = 0usize;
        let indent = if lines.is_empty() { first } else { rest };
        let w = width.saturating_sub(indent.chars().count());
        if !lines.is_empty() && chunks.last().is_some_and(|c| blank(c)) {
            chunks.pop();
        }
        while let Some(c) = chunks.last() {
            if cur_len + c.len() <= w {
                cur_len += c.len();
                cur.push(chunks.pop().unwrap_or_default());
            } else {
                break;
            }
        }
        if chunks.last().is_some_and(|c| c.len() > w) {
            // `_handle_long_word`.
            let space_left = if w < 1 { 1 } else { w - cur_len };
            let chunk = chunks.pop().unwrap_or_default();
            let mut end = space_left;
            if hyphens && chunk.len() > space_left {
                if let Some(h) = chunk[..space_left.min(chunk.len())]
                    .iter()
                    .rposition(|c| *c == '-')
                {
                    if h > 0 && chunk[..h].iter().any(|c| *c != '-') {
                        end = h + 1;
                    }
                }
            }
            let end = end.min(chunk.len());
            cur.push(chunk[..end].to_vec());
            chunks.push(chunk[end..].to_vec());
        }
        if cur.last().is_some_and(|c| blank(c)) {
            cur.pop();
        }
        if !cur.is_empty() {
            let mut line = indent.to_string();
            for c in &cur {
                line.extend(c.iter());
            }
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<&'static str> {
        list.iter()
            .map(|w| &*Box::leak(w.to_string().into_boxed_str()))
            .collect()
    }

    #[test]
    fn the_words() {
        assert_eq!(
            parse(&[]),
            Ask::Look {
                id: String::new(),
                json: false
            }
        );
        assert_eq!(
            parse(&words(&["x", "--json"])),
            Ask::Look {
                id: "x".into(),
                json: true
            }
        );
        assert_eq!(
            parse(&words(&["", "x"])),
            Ask::Look {
                id: "x".into(),
                json: false
            }
        );
        assert_eq!(parse(&words(&["x", "--help"])), Ask::Usage);
        assert_eq!(
            parse(&words(&["--bogus", "--help"])),
            Ask::Bad(format!("Unknown flag: --bogus\n{USAGE}\n"))
        );
        assert_eq!(
            parse(&words(&["-"])),
            Ask::Bad(format!("Unknown flag: -\n{USAGE}\n"))
        );
        assert_eq!(parse(&words(&["x", "y"])), Ask::Bad(format!("{USAGE}\n")));
        // An empty second word is a second word: the id is not empty.
        assert_eq!(parse(&words(&["x", ""])), Ask::Bad(format!("{USAGE}\n")));
        assert!(kit_id("mpc-E001") && kit_id("MPCE-1") && !kit_id("mpx-1") && !kit_id(" mpc-1"));
    }

    #[test]
    fn textwrap_as_python() {
        // Values from Python 3.12's textwrap.
        let t = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho";
        assert_eq!(
            wrap(t, 20, "", "", true),
            vec![
                "alpha beta gamma",
                "delta epsilon zeta",
                "eta theta iota kappa",
                "lambda mu nu xi",
                "omicron pi rho"
            ]
        );
        assert_eq!(
            wrap("a well-known self-evident fact", 10, "", "", true),
            vec!["a well-", "known", "self-", "evident", "fact"]
        );
        assert_eq!(
            wrap("a well-known self-evident fact", 10, "", "", false),
            vec!["a", "well-known", "self-evide", "nt fact"]
        );
        assert_eq!(
            wrap("x -- y and--then", 6, "", "", true),
            vec!["x -- y", "and--", "then"]
        );
        assert_eq!(
            wrap("abcdefghijklmnop", 5, "", "", true),
            vec!["abcde", "fghij", "klmno", "p"]
        );
        assert_eq!(
            wrap("fix: run it", 76, "", "     ", true),
            vec!["fix: run it"]
        );
        assert_eq!(wrap("", 76, "", "", true), Vec::<String>::new());
        assert_eq!(wrap("   ", 76, "", "", true), Vec::<String>::new());
    }

    #[test]
    fn the_kit_s_request_and_answers() {
        assert_eq!(
            kit_body("MPC-E301", false),
            r#"{"argv":["explain","MPC-E301"],"json":false}"#
        );
        assert_eq!(
            kit_body("mpc-\"\\x", true),
            r#"{"argv":["explain","mpc-\"\\x"],"json":true}"#
        );
        let a = kit_answer("200", br#"{"exit": 0, "stdout": "page\n"}"#, false);
        assert_eq!((a.stdout.as_str(), a.stderr.as_str(), a.exit), ("page\n", "", Exit::Ok));
        let row = br#"{"id": "MPC-E110", "code": "unknown_id", "message": "no error id x", "next": "list them", "exit": 4}"#;
        let a = kit_answer("422", row, false);
        assert_eq!(
            (a.stderr.as_str(), a.exit),
            ("Error [unknown_id] MPC-E110: no error id x\nNext: list them\n", Exit::NotFound)
        );
        let a = kit_answer("422", row, true);
        assert_eq!(a.stdout, format!("{}\n", String::from_utf8_lossy(row)));
        let a = kit_answer("000", b"", false);
        assert_eq!(
            (a.stderr, a.exit),
            (
                format!("Error [daemon_unreachable]: no answer from the {} daemon\n", k2_client::brand::PRODUCT_NAME),
                Exit::Error
            )
        );
        let a = kit_answer("000", b"", true);
        assert_eq!(
            a.stdout,
            format!(
                "{{\"error\": {{\"code\": \"daemon_unreachable\", \"hint\": \"no answer from the {} daemon\"}}}}\n",
                k2_client::brand::PRODUCT_NAME
            )
        );
        let off = br#"{"error": {"code": "not_enabled", "hint": "Board files are a preview"}}"#;
        let a = kit_answer("404", off, false);
        assert_eq!(
            (a.stderr.as_str(), a.exit),
            ("Error [not_enabled]: Board files are a preview\n", Exit::Refused)
        );
        let a = kit_answer("500", b"not json", false);
        assert_eq!(
            (a.stderr.as_str(), a.exit),
            ("Error [error]: the daemon refused (HTTP 500)\n", Exit::Error)
        );
    }

    /// The shell's `_explain_kit`: its request and its reader's lines.
    #[test]
    fn the_shell_s_kit_lines() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/explain.sh");
        let sh = std::fs::read_to_string(&p).expect("explain.sh");
        for line in [
            r#"        MPC-*|MPCE-*) _explain_kit "$id" "$json_out"; return $? ;;"#,
            r#"        body="{\"argv\":[\"explain\",${_SETUP_J}],\"json\":true}""#,
            r#"        body="{\"argv\":[\"explain\",${_SETUP_J}],\"json\":false}""#,
            r#"    _compute_split "$(_compute_send POST /cli/board/file/verb "$body")""#,
            r#"        print("Error [%s] %s: %s" % (v.get("code"), v.get("id"), v.get("message")), file=sys.stderr)"#,
            r#"hint = err.get("hint") or ("no answer from the K2 daemon" if status == "000" else "the daemon refused (HTTP %s)" % status)"#,
            r#"sys.exit(3 if code in ("not_enabled", "forbidden", "not_connected", "sandboxed_agent", "core_off") else 1)"#,
        ] {
            assert!(sh.contains(line), "explain.sh no longer has {line:?}");
        }
    }

    #[test]
    fn a_kit_id_is_the_kit_s_explain() {
        let p = Prelude {
            port: true,
            token: true,
            passport: false,
            hook_sock: false,
        };
        let env = Env::of(&[]);
        let args = |l: &[&str]| -> Vec<OsString> { l.iter().map(OsString::from).collect() };
        let w =
            |l: &[&str]| -> Option<Vec<String>> { Some(l.iter().map(|s| s.to_string()).collect()) };
        assert_eq!(
            kit_call(&args(&["explain", "mpc-E001"]), &p, &env),
            w(&["explain", "mpc-E001"])
        );
        assert_eq!(
            kit_call(&args(&["explain", "--json", "MPCE-2"]), &p, &env),
            w(&["explain", "MPCE-2", "--json"])
        );
        assert_eq!(kit_call(&args(&["explain", "mpx-1"]), &p, &env), None);
        assert_eq!(kit_call(&args(&["explain", "mpc-1", "-h"]), &p, &env), None);
        assert_eq!(kit_call(&args(&["explain", "mpc-1", "x"]), &p, &env), None);
        let guarded = Env::of(&[("K2_NO_OWNER_FALLBACK", "1")]);
        let no_token = Prelude { token: false, ..p };
        assert_eq!(
            kit_call(&args(&["explain", "mpc-1"]), &no_token, &guarded),
            None
        );
    }

    #[test]
    fn a_kit_id_goes_to_the_shell() {
        let p = Prelude {
            port: true,
            token: true,
            passport: false,
            hook_sock: false,
        };
        let env = Env::of(&[]);
        let args = |l: &[&str]| -> Vec<OsString> { l.iter().map(OsString::from).collect() };
        assert!(page(&args(&["explain", "mpc-E001"]), &p, &env).is_none());
        assert!(page(&args(&["explain", "--json", "MPCE-2"]), &p, &env).is_none());
        assert!(page(&args(&["explain", "mpx-1"]), &p, &env).is_some());
    }

    #[test]
    fn the_registry_reads() {
        let reg = json::parse(registry()).expect("the registry is JSON");
        assert!(items(key(&reg, "rows")).len() > 100);
        let page = answer(&reg, "  NOPE ", false);
        assert_eq!(page.exit, Exit::NotFound);
        assert!(page.text.contains("id 'nope' ("), "{}", page.text);
    }

    /// Every name a line prints is the brand's (a `K2_BRAND_DIR=brand/xy`
    /// build says `xy`).
    #[test]
    fn the_lines_name_the_brand() {
        let (cli, product) = (k2_client::brand::CLI_NAME, k2_client::brand::PRODUCT_NAME);
        assert_eq!(USAGE, format!("usage: {cli} explain [<id>] [--json]"));
        for line in [LIST_HEAD, ASKABLE, UNKNOWN_HINT] {
            assert!(line.contains(&format!("{cli} ")), "{line}");
            if cli != "k2" {
                assert!(!line.contains("k2"), "{line}");
            }
        }
        let reg = json::parse(registry()).expect("the registry is JSON");
        let list = list(&reg);
        if cli != "k2" {
            assert!(!list.contains("k2 explain"), "{list}");
        }
        let row = json::parse(r#"{"id":"x","who":"k2"}"#).expect("a row");
        assert!(
            row_text(&row).contains(&format!("{product} (wait, retry or update)")),
            "{}",
            row_text(&row)
        );
    }
}
