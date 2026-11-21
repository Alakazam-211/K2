//! `k5 thread ask|secret|withdraw|dismiss|delete` (R3.6), as
//! `cmd_thread_ask`, `cmd_thread_secret`, `cmd_thread_void` and
//! `cmd_thread_delete` do them (`cli/src/browser.sh`).
//!
//! - `ask <addr> "prompt" --option … | --options "a; b; c"`: a card with
//!   choices, sent once by its key (as a write is, [`super::post`]);
//!   `--options` goes as typed (the daemon splits it on `;` only), with
//!   [`OPTIONS_ONE_NOTE`] on stderr when it has a comma and no `;`;
//!   `<id>  card #<seq>`; the markdown hint after it.
//! - `secret <addr> --name NAME`: a card that asks a person for a secret
//!   into the vault. Nothing on this side ever holds the value, and
//!   `--json` drops a `value` or `secret` field all the same.
//! - `withdraw` (`void`) and `dismiss <addr> <card-id> [--why …]`: close a
//!   waiting card; exit 3 for a card that isn't the caller's to close, 4
//!   for none.
//! - `delete <addr> <seq|id>`: a person deletes a message; exit 3 for an
//!   agent or someone else's message, 4 for none.
//!
//! Each verb's words are read before anything is sent; the sender lookup
//! ([`super::post::sender`]) comes before `ask`'s and `secret`'s request.
//! The shell answers a `delete` whose message names a digit outside
//! ASCII (Python's `isdigit`).

use super::super::msg::py::eq;
use super::super::msg::send::echo;
use super::super::msg::send_once::{self, held};
use super::super::Raise;
use super::post::{echo_out, md_nudge, notes, sender};
use super::py::{self, dumps, get_at, get_or, loads, not_ok, or, s};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::py::{str_of, truthy, type_name};
use crate::out::{self};
use crate::prelude::Closing;
use crate::transport::{Call, Client, Error};
use k2_client::brand::CLI_NAME;

/// The routes.
pub const ASK: &str = "/cli/thread/ask";
pub const SECRET: &str = "/cli/thread/secret";
pub const VOID: &str = "/cli/thread/void";
pub const DELETE: &str = "/cli/thread/message/delete";

/// `ask`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    pub target: String,
    pub prompt: String,
    pub json: bool,
    pub from: String,
    pub options_csv: String,
    pub options: Vec<String>,
    pub allow_custom: bool,
    pub expires: String,
    pub no_expiry: bool,
    pub idem_key: String,
    /// `--meta` values, in order.
    pub meta_pairs: Vec<String>,
    /// `--meta-json`'s value, when given.
    pub meta_json: Option<String>,
    /// The merged map ([`super::meta::merge`]): its compact JSON, keys
    /// sorted; `None` for none.
    pub meta: Option<String>,
}

/// `secret`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Secret {
    pub target: String,
    pub json: bool,
    pub from: String,
    pub name: String,
    pub prompt: String,
    /// `--dest`: only `vault` (or empty) is taken.
    pub dest: String,
    /// `scope`, `for`, `delivery`, `kind`, `ttl`, in that order.
    pub vault: [String; 5],
    pub once: bool,
    pub inject_dst: String,
    pub inject_format: String,
}

/// `withdraw`'s and `dismiss`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Void {
    pub verb: String,
    pub target: String,
    pub card: String,
    pub json: bool,
    pub why: String,
}

/// `delete`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delete {
    pub target: String,
    pub which: String,
    pub json: bool,
}

/// A verb's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    Ask(Ask),
    Secret(Secret),
    Void(Void),
    Delete(Delete),
}

/// The words, or the lines and exit that stop the command first.
pub type Parsed = Result<Card, (String, Exit)>;

fn unknown(word: &str) -> String {
    super::post::unknown_flag(word)
}

fn extra(word: &str) -> String {
    format!("Error: unexpected extra argument '{word}'\n")
}

/// `ask`'s usage block's second line.
pub fn ask_usage() -> String {
    format!(
        "Usage: {CLI_NAME} thread ask <addr> \"<prompt>\" --option \"…\" --option \"…\"   (or --options \"a; b; c\")\n"
    )
}

/// The one stderr line `_k2_options_one_note` prints: an `--options`
/// value with a comma and no `;` stayed one option.
pub const OPTIONS_ONE_NOTE: &str =
    "--options splits on ';'; this became one option. Use ';' between options, or repeat --option";

/// `_k2_options_one_note "$1"`: the note when `raw` has a comma and no `;`.
pub fn options_one_note(raw: &str) -> Option<&'static str> {
    (raw.contains(',') && !raw.contains(';')).then_some(OPTIONS_ONE_NOTE)
}

/// `ask`'s note: none for a value whose first character after the
/// shell's `[:space:]` is `[` (a JSON list the daemon reads whole).
pub fn ask_options_note(raw: &str) -> Option<&'static str> {
    let lead = raw.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    if lead.starts_with('[') {
        return None;
    }
    options_one_note(raw)
}

const MIX: &str = "Error: mix of --options and --option is not allowed\n";
const FIRE_AND_FORGET: &str = "Error: thread cards are fire-and-forget; no --wait / --timeout\n";

/// The verb's words (`verb`: the word [`super::first_word`] found).
pub fn parse(verb: &str, words: &[&str]) -> Option<Parsed> {
    match verb {
        "ask" => {
            let mut a = match parse_ask(words) {
                Ok(a) => a,
                Err(stop) => return Some(Err(stop)),
            };
            if a.meta_json.is_none() && a.meta_pairs.is_empty() {
                return Some(Ok(Card::Ask(a)));
            }
            match super::meta::merge(a.meta_json.as_deref(), &a.meta_pairs)? {
                Ok(m) => {
                    a.meta = m;
                    Some(Ok(Card::Ask(a)))
                }
                Err(words) => Some(Err((format!("Error: {words}\n"), Exit::Usage))),
            }
        }
        "secret" => Some(parse_secret(words).map(Card::Secret)),
        "delete" => parse_delete(words).map(|p| p.map(Card::Delete)),
        _ => Some(parse_void(words).map(Card::Void)),
    }
}

fn parse_ask(words: &[&str]) -> Result<Ask, (String, Exit)> {
    let mut a = Ask::default();
    let err = |t: &str| Err((t.to_string(), Exit::Error));
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        let take = |slot: &mut String, line: &str| match next {
            Some(v) => {
                *slot = v.to_string();
                Ok(())
            }
            None => Err((line.to_string(), Exit::Error)),
        };
        match w {
            "ask" => {}
            "--json" => a.json = true,
            "--idempotency-key" => match next {
                Some(k) if !k.is_empty() => {
                    a.idem_key = k.to_string();
                    i += 1;
                }
                _ => return Err((super::post::IDEM_USAGE.into(), Exit::Usage)),
            },
            "--from" => {
                take(&mut a.from, "Error: --from requires a value\n")?;
                i += 1;
            }
            "--options" => {
                if next.is_none() {
                    return err("Error: --options requires a list with ';' between options\n");
                }
                if !a.options.is_empty() {
                    return err(MIX);
                }
                take(&mut a.options_csv, "")?;
                i += 1;
            }
            "--option" => {
                let Some(v) = next else {
                    return err("Error: --option requires a label\n");
                };
                if !a.options_csv.is_empty() {
                    return err(MIX);
                }
                a.options.push(v.to_string());
                i += 1;
            }
            "--allow-custom" => a.allow_custom = true,
            "--expires" => {
                take(
                    &mut a.expires,
                    "Error: --expires requires a time frame (2h, 3d, 90m)\n",
                )?;
                i += 1;
            }
            "--no-expiry" => a.no_expiry = true,
            "--meta" => {
                let Some(v) = next else {
                    return Err((format!("Error: {}\n", super::meta::NEEDS_PAIR), Exit::Usage));
                };
                a.meta_pairs.push(v.to_string());
                i += 1;
            }
            "--meta-json" => {
                let Some(v) = next else {
                    return Err((
                        format!("Error: {}\n", super::meta::NEEDS_OBJECT),
                        Exit::Usage,
                    ));
                };
                if a.meta_json.is_some() {
                    return Err((format!("Error: {}\n", super::meta::JSON_TWICE), Exit::Usage));
                }
                a.meta_json = Some(v.to_string());
                i += 1;
            }
            "--wait" | "--timeout" => return err(FIRE_AND_FORGET),
            "--style" => return err("Error: no --style in v1\n"),
            // The rest is the address and the prompt, as typed.
            "--" => {
                for w in &words[i + 1..] {
                    if a.target.is_empty() {
                        a.target = w.to_string();
                    } else if a.prompt.is_empty() {
                        a.prompt = w.to_string();
                    } else {
                        return err(&extra(w));
                    }
                }
                break;
            }
            w if w.starts_with('-') => return err(&unknown(w)),
            w => {
                if a.target.is_empty() {
                    a.target = w.to_string();
                } else if a.prompt.is_empty() {
                    a.prompt = w.to_string();
                } else {
                    return err(&extra(w));
                }
            }
        }
        i += 1;
    }
    if a.target.is_empty() {
        return Err((
            format!("Error: address required\n{}", ask_usage()),
            Exit::Error,
        ));
    }
    if a.prompt.is_empty() {
        return Err((
            format!("Error: prompt required\n{}", ask_usage()),
            Exit::Error,
        ));
    }
    if a.options_csv.is_empty() && a.options.is_empty() {
        return err("Error: ask requires --option \"…\" (repeatable) or --options \"a; b; c\"\n");
    }
    if !a.expires.is_empty() && a.no_expiry {
        return err("Error: use --expires or --no-expiry, not both\n");
    }
    Ok(a)
}

fn parse_secret(words: &[&str]) -> Result<Secret, (String, Exit)> {
    let mut v = Secret::default();
    let mut header = String::new();
    let err = |t: String| Err((t, Exit::Error));
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        let needs = |line: String| -> Result<String, (String, Exit)> {
            next.map(str::to_string).ok_or((line, Exit::Error))
        };
        let value = |w: &str| needs(format!("Error: {w} requires a value\n"));
        match w {
            "secret" => {}
            "--inject-dst" => {
                v.inject_dst = value(w)?;
                i += 1;
            }
            "--inject-header" => {
                header = value(w)?;
                i += 1;
            }
            "--inject-format" => {
                v.inject_format = value(w)?;
                i += 1;
            }
            "--json" => v.json = true,
            "--from" | "--name" | "--prompt" | "--dest" => {
                let got = value(w)?;
                match w {
                    "--from" => v.from = got,
                    "--name" => v.name = got,
                    "--prompt" => v.prompt = got,
                    _ => v.dest = got,
                }
                i += 1;
            }
            "--scope" | "--for" | "--delivery" | "--kind" | "--ttl" => {
                let at = ["--scope", "--for", "--delivery", "--kind", "--ttl"]
                    .iter()
                    .position(|f| *f == w)
                    .unwrap_or(0);
                v.vault[at] = value(w)?;
                i += 1;
            }
            "--wait" | "--timeout" => return err(FIRE_AND_FORGET.into()),
            "--once" => v.once = true,
            w if w.starts_with('-') => return err(unknown(w)),
            w => {
                if v.target.is_empty() {
                    v.target = w.to_string();
                } else {
                    return err(extra(w));
                }
            }
        }
        i += 1;
    }
    if v.target.is_empty() {
        return err(format!(
            "Error: address required\nUsage: {CLI_NAME} thread secret <addr> --name NAME\n"
        ));
    }
    if v.name.is_empty() {
        return err("Error: secret --name is required\n".into());
    }
    if !v.dest.is_empty() && v.dest != "vault" {
        return err("Error: --dest vault only\n".into());
    }
    if !v.inject_dst.is_empty() && !header.is_empty() {
        return err("Error: give --inject-dst or --inject-header, not both\n".into());
    }
    if !header.is_empty() {
        v.inject_dst = format!("header:{header}");
    }
    Ok(v)
}

fn parse_void(words: &[&str]) -> Result<Void, (String, Exit)> {
    let mut v = Void {
        verb: "withdraw".into(),
        ..Void::default()
    };
    let err = |t: String| Err((t, Exit::Usage));
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "void" | "withdraw" | "dismiss" => {
                if v.target.is_empty() && v.verb == "withdraw" {
                    v.verb = w.to_string();
                }
            }
            "--json" => v.json = true,
            "--why" | "--reason" => {
                let Some(n) = words.get(i + 1) else {
                    return err(format!(
                        "Error: {w} requires a value (one line, at most 200 characters)\n"
                    ));
                };
                v.why = n.to_string();
                i += 1;
            }
            w if w.starts_with('-') => return err(unknown(w)),
            w => {
                if v.target.is_empty() {
                    v.target = w.to_string();
                } else if v.card.is_empty() {
                    v.card = w.to_string();
                } else {
                    return err(extra(w));
                }
            }
        }
        i += 1;
    }
    if v.verb == "void" {
        v.verb = "withdraw".into();
    }
    let usage = format!(
        "Usage: {CLI_NAME} thread {} <addr> <card-id> [--why \"…\"]\n",
        v.verb
    );
    if v.target.is_empty() {
        return err(format!("Error: address required\n{usage}"));
    }
    if v.card.is_empty() {
        return err(format!(
            "Error: card id required (the id {CLI_NAME} thread ask / secret printed)\n{usage}"
        ));
    }
    Ok(v)
}

fn parse_delete(words: &[&str]) -> Option<Result<Delete, (String, Exit)>> {
    let mut d = Delete::default();
    let words = match words.first() {
        Some(&"delete") => &words[1..],
        _ => words,
    };
    let err = |t: String| Some(Err((t, Exit::Usage)));
    for w in words {
        match *w {
            "--json" => d.json = true,
            w if w.starts_with('-') => return err(unknown(w)),
            w => {
                if d.target.is_empty() {
                    d.target = w.to_string();
                } else if d.which.is_empty() {
                    d.which = w.to_string();
                } else {
                    return err(extra(w));
                }
            }
        }
    }
    let usage = format!("Usage: {CLI_NAME} thread delete <addr> <seq|id> [--json]\n");
    if d.target.is_empty() {
        return err(format!("Error: address required\n{usage}"));
    }
    if d.which.is_empty() {
        return err(format!(
            "Error: which message: its number (the [N] in {CLI_NAME} thread <addr>) or its id\n{usage}"
        ));
    }
    if d.which
        .chars()
        .any(|c| c.is_numeric() && !c.is_ascii_digit())
    {
        return None;
    }
    Some(Ok(d))
}

/// The verb (its words read; the lines that stop it printed here).
pub fn run(client: &mut Client, closing: &mut Closing, parsed: Parsed) -> Exit {
    match parsed {
        Err((text, exit)) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            exit
        }
        Ok(Card::Ask(a)) => ask(client, closing, &a),
        Ok(Card::Secret(v)) => secret(client, closing, &v),
        Ok(Card::Void(v)) => void(client, closing, &v),
        Ok(Card::Delete(d)) => delete(client, closing, &d),
    }
}

/// `cli_send cli_post_json <endpoint> <body>` held in `$(…)`: the answer,
/// or the command's end.
fn post_json(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    body: &str,
) -> Result<Vec<u8>, Exit> {
    let passport = client.passport().is_some();
    match client.send(endpoint, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(held(&r.body))
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

fn kv(k: &str, v: Value) -> (Text, Value) {
    (Text::from(k), v)
}

/// `printf '%s\n' "${option_list[@]}"` read back line by line by Python
/// (universal newlines, each line's `\n` stripped).
pub fn option_lines(options: &[String]) -> Vec<String> {
    let mut text = String::new();
    for o in options {
        text.push_str(o);
        text.push('\n');
    }
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    lines.pop();
    lines
}

/// The JSON `ask` sends.
pub fn ask_body(a: &Ask, from: &str, key: &str) -> String {
    let mut o = vec![
        kv("addr", s(&a.target)),
        kv("prompt", s(&a.prompt)),
        kv("from", s(from)),
        kv("idempotency_key", s(key)),
    ];
    if !a.options_csv.is_empty() {
        o.push(kv("options", s(&a.options_csv)));
    }
    if !a.options.is_empty() {
        let list = option_lines(&a.options).iter().map(|l| s(l)).collect();
        o.push(kv("option", Value::Array(list)));
    }
    if a.allow_custom {
        o.push(kv("allow_custom", Value::Bool(true)));
    }
    if !a.expires.is_empty() {
        o.push(kv("expires", s(&a.expires)));
    }
    if a.no_expiry {
        o.push(kv("no_expiry", Value::Bool(true)));
    }
    // `json.loads(os.environ["META"])`: the merged map, keys sorted.
    if let Some(m) = a.meta.as_deref().and_then(|m| loads(m.as_bytes())) {
        o.push(kv("meta", m));
    }
    dumps(&Value::Object(o))
}

fn ask(client: &mut Client, closing: &mut Closing, a: &Ask) -> Exit {
    let from = sender(client, closing, &a.from);
    if let Some(note) = ask_options_note(&a.options_csv) {
        out::stderr_line(note);
    }
    let key = match send_once::init(client, &a.idem_key) {
        Ok(k) => k,
        Err(exit) => return exit,
    };
    let body = ask_body(a, &from, &key.key);
    closing.reset_note();
    let soft = match send_once::send(client, closing, &key, ASK, Call::PostJson(body.as_bytes())) {
        Ok(s) => s,
        Err(exit) => return exit,
    };
    if soft.rc != 0 {
        return Exit::from_curl(soft.rc);
    }
    notes(&soft.out, true);
    if a.json {
        return echo_out(&soft.out);
    }
    let exit = asked(&echo(&soft.out)).print();
    if exit == Exit::Ok {
        md_nudge(client);
    }
    exit
}

/// The error branch the card readers share; `Ok(None)`: `ok` is true.
fn failed(d: &Value, o: &mut py::Shown) -> Result<Option<Value>, Raise> {
    if !not_ok(d, 8)? {
        return Ok(None);
    }
    let (line, code) = py::error_line(d, 9)?;
    o.err(&line);
    Ok(Some(code))
}

/// `ask`'s Python on what `echo` handed it.
pub fn asked(raw: &[u8]) -> py::Shown {
    read_with(raw, 0, |d, o| {
        if failed(d, o)?.is_some() {
            return Ok(2);
        }
        let seq = get_at(d, "seq", 17)?;
        let id = get_or(d, "id", s(""), 18)?;
        if matches!(seq, Value::Null) {
            o.line(&str_of(&id));
        } else {
            o.line(&format!("{}  card #{}", str_of(&id), str_of(&seq)));
        }
        Ok(0)
    })
}

/// A reader: not JSON is written back as it came, exit `not_json`; else
/// `f` on the value.
fn read_with(
    raw: &[u8],
    not_json: i32,
    f: impl FnOnce(&Value, &mut py::Shown) -> Result<i32, Raise>,
) -> py::Shown {
    let mut o = py::Shown::default();
    let Some(d) = loads(raw) else {
        o.stdout = raw.to_vec();
        return o.exit(not_json);
    };
    match f(&d, &mut o) {
        Ok(code) => o.exit(code),
        Err(r) => o.raised(&r),
    }
}

/// The JSON `secret` sends.
pub fn secret_body(v: &Secret, from: &str) -> String {
    let mut o = vec![
        kv("addr", s(&v.target)),
        kv("name", s(&v.name)),
        kv("from", s(from)),
        kv("dest", s("vault")),
    ];
    if !v.prompt.is_empty() {
        o.push(kv("prompt", s(&v.prompt)));
    }
    for (k, val) in ["scope", "for", "delivery", "kind", "ttl"]
        .iter()
        .zip(&v.vault)
    {
        if !val.is_empty() {
            o.push(kv(k, s(val)));
        }
    }
    if v.once {
        o.push(kv("once", Value::Bool(true)));
    }
    let mut inj = Vec::new();
    if !v.inject_dst.is_empty() {
        inj.push(kv("dst", s(&v.inject_dst)));
    }
    if !v.inject_format.is_empty() {
        inj.push(kv("fmt", s(&v.inject_format)));
    }
    if !inj.is_empty() {
        o.push(kv("inject", Value::Object(inj)));
    }
    dumps(&Value::Object(o))
}

fn secret(client: &mut Client, closing: &mut Closing, v: &Secret) -> Exit {
    let from = sender(client, closing, &v.from);
    let body = secret_body(v, &from);
    let response = match post_json(client, closing, SECRET, &body) {
        Ok(r) => r,
        Err(exit) => return exit,
    };
    notes(&response, false);
    let raw = echo(&response);
    if v.json {
        secret_json(&raw).print()
    } else {
        secret_shown(&raw).print()
    }
}

/// `secret --json`'s Python: the answer without a `value` or `secret`
/// field.
pub fn secret_json(raw: &[u8]) -> py::Shown {
    read_with(raw, 0, |d, o| {
        let Value::Object(pairs) = d else {
            let e = match d {
                Value::Array(_) => "TypeError: pop expected at most 1 argument, got 2".to_string(),
                other => format!(
                    "AttributeError: '{}' object has no attribute 'pop'",
                    type_name(other)
                ),
            };
            return Err(Raise::at(8, e));
        };
        let kept: Vec<(Text, Value)> = pairs
            .iter()
            .filter(|(k, _)| k.as_str() != Some("value") && k.as_str() != Some("secret"))
            .cloned()
            .collect();
        o.line(&dumps(&Value::Object(kept)));
        Ok(0)
    })
}

/// `secret`'s Python: the card's id on stdout, what it sends on stderr.
pub fn secret_shown(raw: &[u8]) -> py::Shown {
    read_with(raw, 0, |d, o| {
        if failed(d, o)?.is_some() {
            return Ok(2);
        }
        o.line(&str_of(&get_or(d, "id", s(""), 15)?));
        let sends = get_at(d, "sends", 17)?;
        if truthy(&sends) {
            o.err(&format!("card: {}\n", str_of(&sends)));
        }
        Ok(0)
    })
}

/// The JSON `withdraw` and `dismiss` send.
pub fn void_body(v: &Void) -> String {
    let mut o = vec![kv("addr", s(&v.target)), kv("id", s(&v.card))];
    if !v.why.is_empty() {
        o.push(kv("reason", s(&v.why)));
    }
    dumps(&Value::Object(o))
}

fn void(client: &mut Client, closing: &mut Closing, v: &Void) -> Exit {
    let response = match post_json(client, closing, VOID, &void_body(v)) {
        Ok(r) => r,
        Err(exit) => return exit,
    };
    notes(&response, false);
    if v.json {
        return echo_out(&response);
    }
    voided(&echo(&response)).print()
}

/// The exit for a refusing code: 3 for `refusing`, 4 for `not_found`,
/// else 2.
fn code_exit(code: &Value, refusing: &[&str]) -> i32 {
    if refusing.iter().any(|c| eq(code, &s(c))) {
        3
    } else if eq(code, &s("not_found")) {
        4
    } else {
        2
    }
}

/// `withdraw`'s and `dismiss`'s Python.
pub fn voided(raw: &[u8]) -> py::Shown {
    read_with(raw, 0, |d, o| {
        if let Some(code) = failed(d, o)? {
            return Ok(code_exit(
                &code,
                &[
                    "card_not_yours",
                    "action_card_person_only",
                    "forbidden",
                    "card_not_pending",
                    "request_not_pending",
                ],
            ));
        }
        let id = get_or(d, "id", s(""), 23)?;
        let how = or(get_at(d, "closedAs", 23)?, get_or(d, "status", s(""), 23)?);
        o.line(&format!("{}  {}", str_of(&id), str_of(&how)));
        Ok(0)
    })
}

/// The JSON `delete` sends: `seq` for a number (a leading `#` and spaces
/// dropped), else `id`.
pub fn delete_body(d: &Delete) -> String {
    let w = d
        .which
        .trim_matches(super::super::py_space)
        .trim_start_matches('#');
    let mut o = vec![kv("addr", s(&d.target))];
    if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) {
        let n = w.trim_start_matches('0');
        o.push(kv(
            "seq",
            Value::Int(if n.is_empty() { "0" } else { n }.into()),
        ));
    } else {
        o.push(kv("id", s(w)));
    }
    dumps(&Value::Object(o))
}

fn delete(client: &mut Client, closing: &mut Closing, d: &Delete) -> Exit {
    let response = match post_json(client, closing, DELETE, &delete_body(d)) {
        Ok(r) => r,
        Err(exit) => return exit,
    };
    notes(&response, false);
    deleted(&echo(&response), d.json).print()
}

/// `delete`'s Python (`JSON_OUT`: `--json`).
pub fn deleted(raw: &[u8], as_json: bool) -> py::Shown {
    read_with(raw, 2, |d, o| {
        if as_json {
            o.line(py::strip(&String::from_utf8_lossy(raw)));
        }
        if !not_ok(d, 12)? {
            if !as_json {
                let already = truthy(&get_at(d, "already", 30)?);
                let note = if already {
                    "already deleted"
                } else {
                    "deleted; the agent was told (never what it said)"
                };
                let seq = get_or(d, "seq", s(""), 31)?;
                let id = get_or(d, "id", s(""), 31)?;
                o.line(&format!("[{}] {}  {note}", str_of(&seq), str_of(&id)));
            }
            return Ok(0);
        }
        let err = or(get_at(d, "error", 13)?, d.clone());
        let mut code = s("");
        if let Value::Object(_) = err {
            code = or(get_at(&err, "code", 16)?, s("error"));
            if !as_json {
                let hint = or(get_at(&err, "hint", 18)?, err.clone());
                o.err(&format!("{}: {}\n", str_of(&code), str_of(&hint)));
            }
        } else if !as_json {
            o.err(&format!("{}\n", str_of(&err)));
        }
        Ok(code_exit(
            &code,
            &[
                "person_only",
                "not_your_message",
                "not_a_persons_message",
                "forbidden",
                "imported_read_only",
            ],
        ))
    })
}
