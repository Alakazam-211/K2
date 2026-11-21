//! A mailbox's out-of-office reply and the server's footer: `hostmail
//! ooo` (set, unset, show) and `hostmail footer` (set, unset, show), as
//! `cli/src/hostmail-ooo-footer.sh` reads the words and `_mail_py`'s
//! verbs answer (`cli/src/mail.sh`).
//!
//! `--text` is the text itself, `@<path>` (the file's text) or `-` (stdin),
//! read by `_hostmail_read_text` inside `$(…)`, so its trailing newlines
//! are dropped. `-` is read when the verb runs, past the gates
//! ([`STDIN`]); a second `-` reads what the first left: nothing. The
//! binary hands to the shell an `@<path>` file that holds a NUL, is not
//! UTF-8 or can't be read, and a `--days` past bash's 64-bit integers.
//!
//! Text on stdin that is not UTF-8 is held as Python's environment holds
//! it (each bad byte its surrogate escape), so the request's
//! `dumps(body).encode()` raises as the shell's does, nothing sent. Where
//! this differs from the shell: text on stdin with a NUL (bash drops it
//! and, from bash 4.4, warns on stderr naming its own line; this drops it
//! quietly: `scripts/cli-parity.sh`'s named difference
//! `nul-in-stdin-text`). Once stdin is read the line can't be handed on.

use super::words::{scan, wants_help, Spec};
use crate::families::mail::core::{self, run_env, usage, At, Plan, Py, MODULE, R};
use crate::families::mail::py::{self as p, attr_get};
use crate::out::json::Value;
use crate::out::py;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["ooo", "footer"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const OOO_GET_CALL: u32 = 2583;
    pub const OOO_GET_ON: u32 = 2587;
    pub const OOO_GET_UNTIL: u32 = 2588;
    pub const OOO_GET_ADDR: u32 = 2590;
    pub const OOO_GET_DAYS: u32 = 2591;
    pub const OOO_GET_TEXT: u32 = 2593;
    pub const OOO_GET_OFF: u32 = 2596;
    pub const OOO_SET_DAYS: u32 = 2601;
    pub const OOO_SET_CALL: u32 = 2604;
    pub const OOO_SET_ADDR: u32 = 2609;
    pub const OOO_SET_DAYS_PRINT: u32 = 2610;
    pub const OOO_SET_UNTIL: u32 = 2611;
    pub const OOO_UNSET_CALL: u32 = 2614;
    pub const OOO_UNSET_PRINT: u32 = 2618;
    pub const FOOTER_GET_CALL: u32 = 2621;
    pub const FOOTER_GET_ON: u32 = 2625;
    pub const FOOTER_SET_CALL: u32 = 2635;
    pub const FOOTER_UNSET_CALL: u32 = 2645;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "ooo" => ooo(words),
        "footer" => footer(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// Set (to `1`) where `--text -` was the last `--text`: the verb reads
/// stdin for `M_TEXT` when it runs ([`text`]).
pub const STDIN: &str = "M_TEXT_STDIN";

/// What a `--text` gives.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Text {
    Given(String),
    /// `-`: stdin, read when the verb runs.
    Stdin,
}

/// The last `--text`: what it gives, or nothing where a `-` before it
/// had read stdin already.
#[derive(Debug, Default)]
struct Texts {
    last: Option<Text>,
    stdin_read: bool,
}

impl Texts {
    fn take(&mut self, t: Text) {
        if self.last == Some(Text::Stdin) {
            self.stdin_read = true;
        }
        self.last = Some(match t {
            Text::Stdin if self.stdin_read => Text::Given(String::new()),
            t => t,
        });
    }

    /// `M_TEXT` and [`STDIN`] for the verb, or `None` with no `--text`.
    fn env(self) -> Option<(String, &'static str)> {
        match self.last? {
            Text::Given(t) => Some((t, "")),
            Text::Stdin => Some((String::new(), "1")),
        }
    }
}

/// `env("M_TEXT")`: the text; where it was `--text -`, stdin read now
/// (`$(cat)`: NULs and trailing newlines dropped), as Python reads those
/// bytes from its environment: UTF-8, a byte that isn't kept as its
/// surrogate escape, which `dumps(body).encode()` then refuses
/// ([`core::encode_body`]).
fn text(py_: &mut Py) -> Value {
    if py_.env(STDIN) == "1" {
        let all = crate::prompt::read_all_stdin();
        return crate::confirm::text_of_bytes(all.expose());
    }
    sv(py_.env("M_TEXT"))
}

/// `text="$(_hostmail_read_text "$raw")"`: the text, the usage line, or
/// the shell's (`Err(Plan::Shell)`).
fn read_text(raw: &str) -> Result<Text, Plan> {
    if raw == "-" {
        return Ok(Text::Stdin);
    }
    let Some(path) = raw.strip_prefix('@') else {
        return Ok(Text::Given(raw.trim_end_matches('\n').to_string()));
    };
    if path.is_empty() {
        return Err(usage("--text @path needs a path"));
    }
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Err(usage(format!("no such file: {path}")));
    }
    let bytes = std::fs::read(path).map_err(|_| Plan::Shell)?;
    if bytes.contains(&0) {
        return Err(Plan::Shell);
    }
    let text = String::from_utf8(bytes).map_err(|_| Plan::Shell)?;
    Ok(Text::Given(text.trim_end_matches('\n').to_string()))
}

/// A value flag's value, or its usage line.
fn value<'a>(words: &[&'a str], i: usize, missing: &str) -> Result<&'a str, Plan> {
    words.get(i + 1).copied().ok_or_else(|| usage(missing))
}

/// `cmd_hostmail_ooo`.
fn ooo(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/ooo"));
    };
    match *sub {
        "set" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/ooo/set"));
            }
            ooo_set(rest)
        }
        "unset" | "show" => {
            if wants_help(rest) {
                return Ok(Plan::Help(if *sub == "unset" {
                    "hostmail/ooo/unset"
                } else {
                    "hostmail/ooo/show"
                }));
            }
            let use_ = format!("{CLI_NAME} hostmail ooo {sub} <addr>");
            let g = scan(
                rest,
                &Spec {
                    slots: &["address"],
                    values: &[("--address", "address", "--address requires a value")],
                    switches: &[],
                    unknown: &format!("unknown flag '{{w}}' — usage: {use_}"),
                    extra: "unexpected extra argument '{w}' — one address only",
                },
            )?;
            if g.get("address").is_empty() {
                return Err(usage(use_));
            }
            let verb = if *sub == "unset" {
                "ooo_unset"
            } else {
                "ooo_get"
            };
            Ok(Plan::Run(
                run_env(verb, g.json).with("M_ADDRESS", g.get("address")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/ooo")),
        other => Err(usage(format!(
            "unknown ooo command '{other}' — usage: {CLI_NAME} hostmail ooo <set|unset|show>"
        ))),
    }
}

/// `cmd_hostmail_ooo_set`.
fn ooo_set(words: &[&str]) -> Result<Plan, Plan> {
    let use_ = format!(
        "{CLI_NAME} hostmail ooo set <addr> --text FILE_OR_STRING [--days N] [--until DATE]"
    );
    let (mut address, mut text, mut days, mut until) = (
        String::new(),
        Texts::default(),
        String::new(),
        String::new(),
    );
    let mut json = false;
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => json = true,
            "--text" => {
                let raw = value(words, i, "--text requires a value (literal, @path, or -)")?;
                text.take(read_text(raw)?);
                i += 1;
            }
            "--days" => {
                days = value(words, i, "--days requires 1-31")?.to_string();
                i += 1;
            }
            "--until" => {
                until = value(words, i, "--until requires UTC YYYY-MM-DD")?.to_string();
                i += 1;
            }
            "--address" => {
                address = value(words, i, "--address requires a value")?.to_string();
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(format!("unknown flag '{f}' — usage: {use_}")));
            }
            w => {
                if address.is_empty() {
                    address = w.to_string();
                } else {
                    return Err(usage(format!(
                        "unexpected extra argument '{w}' — one address only"
                    )));
                }
            }
        }
        i += 1;
    }
    if address.is_empty() {
        return Err(usage(use_));
    }
    let Some((text, stdin)) = text.env() else {
        return Err(usage("ooo set needs --text (literal, @path, or -)"));
    };
    if !days.is_empty() {
        if core::is_uint(&days) && !core::fits_bash(&days) {
            return Err(Plan::Shell);
        }
        if !core::is_uint(&days) || core::bash_num(&days) < 1 || core::bash_num(&days) > 31 {
            return Err(usage("--days must be 1-31"));
        }
    }
    Ok(Plan::Run(
        run_env("ooo_set", json)
            .with("M_ADDRESS", address)
            .with("M_TEXT", text)
            .with(STDIN, stdin)
            .with("M_DAYS", days)
            .with("M_UNTIL", until),
    ))
}

/// `cmd_hostmail_footer`.
fn footer(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/footer"));
    };
    let page = match *sub {
        "set" => "hostmail/footer/set",
        "unset" => "hostmail/footer/unset",
        "show" => "hostmail/footer/show",
        "--help" | "-h" => return Ok(Plan::Help("hostmail/footer")),
        other => {
            return Err(usage(format!(
            "unknown footer command '{other}' — usage: {CLI_NAME} hostmail footer <set|unset|show>"
        )))
        }
    };
    if wants_help(rest) {
        return Ok(Plan::Help(page));
    }
    let use_ = match *sub {
        "set" => format!("{CLI_NAME} hostmail footer set --text FILE_OR_STRING"),
        other => format!("{CLI_NAME} hostmail footer {other}"),
    };
    let reserved = "--domain is reserved (footer is host-wide)";
    let (mut text, mut domain, mut json) = (Texts::default(), String::new(), false);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--json" => json = true,
            "--text" if *sub == "set" => {
                let raw = value(rest, i, "--text requires a value (literal, @path, or -)")?;
                text.take(read_text(raw)?);
                i += 1;
            }
            "--domain" if *sub == "show" => {
                return Err(usage("--domain is reserved — footer is host-wide this cut"));
            }
            "--domain" => {
                domain = value(rest, i, reserved)?.to_string();
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(format!("unknown flag '{f}' — usage: {use_}")));
            }
            w => {
                return Err(usage(format!("unexpected argument '{w}' — usage: {use_}")));
            }
        }
        i += 1;
    }
    Ok(Plan::Run(match *sub {
        "set" => {
            let Some((text, stdin)) = text.env() else {
                return Err(usage("footer set needs --text (literal, @path, or -)"));
            };
            run_env("footer_set", json)
                .with("M_TEXT", text)
                .with(STDIN, stdin)
                .with("M_DOMAIN", domain)
        }
        "unset" => run_env("footer_unset", json).with("M_DOMAIN", domain),
        _ => run_env("footer_get", json),
    }))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "ooo_get" => ooo_get_py(py_),
        "ooo_set" => ooo_set_py(py_),
        "ooo_unset" => ooo_unset_py(py_),
        "footer_get" => footer_get_py(py_),
        "footer_set" | "footer_unset" => footer_change_py(py_, verb == "footer_set"),
        _ => return None,
    })
}

fn g(d: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(d, key).at(l, MODULE)
}

fn sv(t: &str) -> Value {
    p::sv(t)
}

/// `d.get("address") or env("M_ADDRESS") or "?"` at `l`.
fn addr(py_: &Py, d: &Value, l: u32) -> R<String> {
    Ok(match g(d, "address", l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None if !py_.env("M_ADDRESS").is_empty() => py_.env("M_ADDRESS").to_string(),
        None => "?".into(),
    })
}

/// `d.get(k) if d.get(k) is not None else default` at `l`.
fn or_none(d: &Value, key: &str, default: &str, l: u32) -> R<String> {
    Ok(match g(d, key, l)? {
        None | Some(Value::Null) => default.into(),
        Some(v) => py::str_of(&v),
    })
}

fn ooo_get_py(py_: &mut Py) -> R<()> {
    let params = vec![("address".to_string(), py_.env("M_ADDRESS").to_string())];
    let d = py_.get("/cli/mail/ooo", &params, line::OOO_GET_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    if p::truthy(g(&d, "enabled", line::OOO_GET_ON)?.as_ref()) {
        let until = p::or(g(&d, "until", line::OOO_GET_UNTIL)?, sv("none"));
        let a = addr(py_, &d, line::OOO_GET_ADDR)?;
        let days = or_none(&d, "days", "?", line::OOO_GET_DAYS)?;
        py_.print(&format!(
            "{a}  ooo on  days={days}  until={}",
            py::str_of(&until)
        ));
        if p::truthy(g(&d, "text", line::OOO_GET_TEXT)?.as_ref()) {
            let t = p::py_index(&d, "text").at(line::OOO_GET_TEXT + 1, MODULE)?;
            py_.print_v(&t);
        }
    } else {
        let a = addr(py_, &d, line::OOO_GET_OFF)?;
        py_.print(&format!("{a}  ooo off"));
    }
    Ok(())
}

fn ooo_set_py(py_: &mut Py) -> R<()> {
    let mut body = vec![
        ("address", sv(py_.env("M_ADDRESS"))),
        ("text", text(py_)),
    ];
    if !py_.env("M_DAYS").is_empty() {
        let n = p::py_int(&sv(py_.env("M_DAYS"))).at(line::OOO_SET_DAYS, MODULE)?;
        body.push(("days", Value::Int(n)));
    }
    if !py_.env("M_UNTIL").is_empty() {
        body.push(("until", sv(py_.env("M_UNTIL"))));
    }
    let d = py_.post("/cli/mail/ooo", &core::obj(body), line::OOO_SET_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = addr(py_, &d, line::OOO_SET_ADDR)?;
    let days = or_none(&d, "days", "7", line::OOO_SET_DAYS_PRINT)?;
    let until = p::or(g(&d, "until", line::OOO_SET_UNTIL)?, sv("none"));
    py_.print(&format!(
        "ooo set for {a}  days={days}  until={}",
        py::str_of(&until)
    ));
    Ok(())
}

fn ooo_unset_py(py_: &mut Py) -> R<()> {
    let body = core::obj(vec![("address", sv(py_.env("M_ADDRESS")))]);
    let d = py_.post("/cli/mail/ooo/unset", &body, line::OOO_UNSET_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = addr(py_, &d, line::OOO_UNSET_PRINT)?;
    py_.print(&format!("ooo unset for {a}"));
    Ok(())
}

fn footer_get_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/footer", &[], line::FOOTER_GET_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::FOOTER_GET_ON;
    if p::truthy(g(&d, "enabled", l)?.as_ref()) && p::truthy(g(&d, "text", l)?.as_ref()) {
        py_.print("footer on");
        let t = p::py_index(&d, "text").at(l + 2, MODULE)?;
        py_.print_v(&t);
    } else {
        py_.print("footer off");
    }
    Ok(())
}

fn footer_change_py(py_: &mut Py, set: bool) -> R<()> {
    let mut body = Vec::new();
    if set {
        body.push(("text", text(py_)));
    }
    if !py_.env("M_DOMAIN").is_empty() {
        body.push(("domain", sv(py_.env("M_DOMAIN"))));
    }
    let (route, l) = if set {
        ("/cli/mail/footer", line::FOOTER_SET_CALL)
    } else {
        ("/cli/mail/footer/unset", line::FOOTER_UNSET_CALL)
    };
    let d = py_.post(route, &core::obj(body), l)?;
    if py_.json {
        py_.print_json(&d);
    } else {
        py_.print(if set { "footer set" } else { "footer unset" });
    }
    Ok(())
}
