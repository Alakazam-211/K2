//! Garden data: a widget's own keyspace on this computer, as the shell
//! CLI's `cmd_store_widget` answers it (`cli/src/garden-data.sh`), for
//! `k5 store <verb> --widget <name> …`, and the parts `k5 zen data`
//! shares with it ([`super::super::zen`]).
//!
//! Every call goes through `_compute_send` ([`crate::families::board::send`]):
//! the cell socket first with a passport, then TCP, no note and no
//! bulletin. `_gd_call`'s rule: any status but 200 is `_gd_fail` ([`Gd::fail`]),
//! which prints the daemon's error and ends with the contract's code (3 a
//! person or grant refusal, 4 not found, 5 a conflict, 1 anything else).
//!
//! `--json` prints each answer as it came; otherwise the shell's Python
//! readers print it, and where a reader raises, its traceback (Python
//! 3.12's) is printed and the command ends with 1.
//!
//! Bash bugs copied, under `set -e` (reported):
//! - `changes`: `_gd_changes_page`'s `return 9` on a resync ends the
//!   command with 9 before the resync lines print;
//! - `changes`: a page with no changes ends the command with 1 (its
//!   `grep -v '^#'` finds no line under `pipefail`), so `--follow` stops
//!   at the first quiet 30 seconds.

use crate::exit::Exit;
use crate::families::board::{self, Sent};
use crate::families::{raised, Raise, Shown};
use crate::out::json::{self as js, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::{json as pyjson, ops};
use crate::transport::encode::quote;
use crate::transport::urllib::urlencode;
use crate::transport::Client;
use k2_client::{cli_name, product_name};
use std::io::Write;

#[cfg(test)]
mod tests;

/// `_GD_BLOB_CHUNK`: bytes go in 2 MiB chunks.
pub const BLOB_CHUNK: u64 = 2_097_152;

/// The command ends here, with this exit, after what it printed.
pub type R<T> = Result<T, Exit>;

/// What a reader printed, or what it printed before it raised and the
/// exception.
pub type Read = Result<Shown, (String, String, Raise)>;

/// Writes bytes to stdout; a broken pipe ends the process as the shell's.
pub fn say(bytes: &[u8]) -> R<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    crate::out::write_flushed(&mut std::io::stdout().lock(), bytes)
        .map_err(|e| crate::out::after_stdout_error(&e))
}

/// Writes text to stderr.
pub fn tell(text: &str) {
    let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

/// `echo "Error: …" >&2; exit 2`.
pub fn usage(text: &str) -> Exit {
    tell(&format!("Error: {text}\n"));
    Exit::Usage
}

/// What a reader printed, its exit printed too: `Err` when it was not 0.
pub fn shown(s: Shown) -> R<()> {
    match s.print() {
        Exit::Ok => Ok(()),
        e => Err(e),
    }
}

/// A reader's result as the shell's Python prints it.
pub fn reader(r: Read) -> R<()> {
    match r {
        Ok(s) => shown(s),
        Err((o, e, x)) => shown(raised(o, e, &x)),
    }
}

/// `json.loads(sys.stdin.read())` at `line`.
pub fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    pyjson::loads(&String::from_utf8_lossy(body)).map_err(|e| Raise::at(line, e.last_line()))
}

/// `v[k]`.
pub fn at<'a>(v: &'a Value, k: &str, line: u32) -> Result<&'a Value, Raise> {
    ops::index(v, k, line)
}

/// `json.dumps(v, …)`. A lone surrogate the printer can't write as UTF-8
/// is written escaped (Python would fail writing it out).
pub fn dumps(v: &Value, options: &Options) -> String {
    js::to_string(v, options).unwrap_or_else(|_| {
        js::to_string(
            v,
            &Options {
                ensure_ascii: true,
                ..options.clone()
            },
        )
        .unwrap_or_default()
    })
}

/// `json.dumps(v)`: Python's defaults.
pub fn dumps_default(v: &Value) -> String {
    dumps(v, &Options::default())
}

/// `json.dumps(v, ensure_ascii=False)`.
pub fn dumps_text(v: &Value) -> String {
    dumps(
        v,
        &Options {
            ensure_ascii: false,
            ..Options::default()
        },
    )
}

/// `json.dumps(v, indent=2, ensure_ascii=False)`.
pub fn dumps_indented(v: &Value) -> String {
    dumps(
        v,
        &Options {
            ensure_ascii: false,
            indent: Some(2),
            ..Options::default()
        },
    )
}

/// `json.dumps(v, separators=(",", ":"), ensure_ascii=…)`.
pub fn dumps_compact(v: &Value, ensure_ascii: bool) -> String {
    dumps(
        v,
        &Options {
            compact: true,
            ensure_ascii,
            ..Options::default()
        },
    )
}

/// `'%-<w>s' % v`.
pub fn left(v: &Value, w: usize) -> String {
    crate::families::py_left(v, w)
}

/// `'%d' % v` at `line`.
pub fn d(v: &Value, line: u32) -> Result<String, Raise> {
    crate::families::py_d(v, line)
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `int(digits)` for a word bash already checked is all digits.
pub fn int(digits: &str) -> Value {
    let t = digits.trim_start_matches('0');
    Value::Int(if t.is_empty() { "0".into() } else { t.into() })
}

/// A dict that keeps Python's insertion order: setting a key that is
/// there keeps its place.
#[derive(Debug, Default, Clone)]
pub struct Dict(pub Vec<(Text, Value)>);

impl Dict {
    pub fn set(&mut self, k: &str, v: Value) {
        match self.0.iter_mut().find(|(key, _)| key.as_str() == Some(k)) {
            Some(slot) => slot.1 = v,
            None => self.0.push((Text::from(k), v)),
        }
    }

    pub fn has(&self, k: &str) -> bool {
        self.0.iter().any(|(key, _)| key.as_str() == Some(k))
    }

    pub fn value(self) -> Value {
        Value::Object(self.0)
    }
}

/// `str(x)` as `print` writes it, trailing newlines dropped (`$(…)`).
fn captured(v: &Value) -> String {
    py::str_of(v).trim_end_matches('\n').to_string()
}

/// One Garden data run: the client, `--json` and the widget.
pub struct Gd<'a> {
    pub client: &'a mut Client,
    pub json: bool,
    pub widget: String,
}

/// The error's code, as `_gd_fail`'s first reader prints it.
pub fn fail_code(body: &[u8]) -> String {
    let Ok(v) = pyjson::loads(&String::from_utf8_lossy(body)) else {
        return String::new();
    };
    let Value::Object(pairs) = &v else {
        return String::new();
    };
    let code = match crate::families::get(pairs, "error") {
        Some(Value::Object(e)) => crate::families::get(e, "code")
            .map(py::str_of)
            .unwrap_or_default(),
        Some(e) if py::truthy(e) => py::str_of(e),
        _ => String::new(),
    };
    code.trim_end_matches('\n').to_string()
}

/// `_gd_fail`'s second reader: the error line.
pub fn fail_line(body: &[u8]) -> String {
    let raw = String::from_utf8_lossy(body);
    let read = pyjson::loads(&raw).ok().and_then(|v| match v {
        Value::Object(pairs) => Some(crate::families::get(&pairs, "error").cloned()),
        _ => None,
    });
    match read {
        Some(Some(Value::Object(e))) => {
            let code = crate::families::get(&e, "code")
                .map(py::str_of)
                .unwrap_or_else(|| "error".into());
            let hint = crate::families::get(&e, "hint")
                .map(py::str_of)
                .unwrap_or_default();
            format!("Error: {code}: {hint}")
        }
        Some(e) => format!("Error: {}", py::str_of(&e.unwrap_or(Value::Null))),
        None => {
            let head: String = raw.chars().take(200).collect();
            format!("Error: daemon returned: {head}")
        }
    }
}

/// `_gd_fail`'s exit for a code.
pub fn fail_exit(code: &str) -> Exit {
    match code {
        "person_only" | "owner_only" => {
            tell(&format!(
                "This needs a person. Run it outside {}, or use Settings → Gardens → the Garden → Local Data.\n",
                product_name!()
            ));
            Exit::Refused
        }
        "not_granted" | "self_grant" | "level_too_high" | "not_own" | "author_unknown"
        | "core_off" => Exit::Refused,
        "space_not_found" | "agent_not_found" | "project_not_found" | "grant_not_found"
        | "widget_not_found" | "no_snapshot" | "blob_missing" => Exit::NotFound,
        "conflict" => Exit::Conflict,
        _ => Exit::Error,
    }
}

impl Gd<'_> {
    /// `_gd_fail STATUS BODY`: the error, then the exit.
    pub fn fail(&self, status: &str, body: &[u8]) -> Exit {
        let code = fail_code(body);
        if self.json && !body.is_empty() {
            let mut b = body.to_vec();
            b.push(b'\n');
            if let Err(e) = say(&b) {
                return e;
            }
        } else if !body.is_empty() {
            tell(&format!("{}\n", fail_line(body)));
        } else {
            let status = if status.is_empty() { "none" } else { status };
            tell(&format!(
                "Error: no answer from the {} daemon (status {status})\n",
                product_name!()
            ));
        }
        fail_exit(&code)
    }

    /// `_compute_send` in `$(…)`, split.
    pub fn send(&mut self, post: bool, path: &str, payload: &str) -> R<Sent> {
        board::send(self.client, post, path, payload)
    }

    /// `_gd_call METHOD PATH PAYLOAD`: the answer's body on a 200.
    pub fn call(&mut self, post: bool, path: &str, payload: &str) -> R<Vec<u8>> {
        let sent = self.send(post, path, payload)?;
        if sent.status != "200" {
            return Err(self.fail(&sent.status, &sent.resp));
        }
        Ok(sent.resp)
    }

    /// `_gd_q key=value …`: the widget first, then each pair with a value.
    pub fn q(&self, pairs: &[(&str, &str)]) -> String {
        let mut q = format!("widget={}", quote(&self.widget));
        for (k, v) in pairs {
            if !v.is_empty() {
                q.push_str(&format!("&{k}={}", quote(v)));
            }
        }
        q
    }

    /// `_gd_out READER`: the body as it came with `--json`, else what the
    /// reader makes of it.
    pub fn out(&self, body: &[u8], read: impl FnOnce(&[u8]) -> Read) -> R<()> {
        if self.json {
            let mut b = body.to_vec();
            b.push(b'\n');
            return say(&b);
        }
        reader(read(body))
    }
}

/// A reader with only stdout lines, from one value.
pub fn lines(
    body: &[u8],
    line: u32,
    f: impl FnOnce(&Value, &mut String, &mut String) -> Result<(), Raise>,
) -> Read {
    let v = load(body, line).map_err(|r| (String::new(), String::new(), r))?;
    let (mut o, mut e) = (String::new(), String::new());
    match f(&v, &mut o, &mut e) {
        Ok(()) => Ok(Shown {
            stdout: o,
            stderr: e,
            exit: Exit::Ok,
        }),
        Err(r) => Err((o, e, r)),
    }
}

/// `_gd_args`'s flags.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Args {
    pub pos: String,
    pub pos2: String,
    pub value: String,
    pub has_value: bool,
    pub file: String,
    pub ifrev: String,
    pub prefix: String,
    pub after: String,
    pub limit: String,
    pub all: bool,
    pub since: String,
    pub wait: String,
    pub follow: bool,
    pub agent: String,
    pub project: String,
    pub level: String,
    pub yes: bool,
}

/// `_gd_args "$@"`: the flags, or the usage line's text.
pub fn gd_args(words: &[String], json: &mut bool) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let val = |why: &str| -> Result<String, String> {
            match words.get(i + 1) {
                Some(v) => Ok(v.clone()),
                None => Err(why.to_string()),
            }
        };
        let slot = match w {
            "--value" => Some((&mut a.value, "--value takes JSON")),
            "--file" => Some((&mut a.file, "--file takes a path")),
            "--if-rev" => Some((&mut a.ifrev, "--if-rev takes a number")),
            "--prefix" => Some((&mut a.prefix, "--prefix takes a prefix")),
            "--after" => Some((&mut a.after, "--after takes a key")),
            "--limit" => Some((&mut a.limit, "--limit takes a number")),
            "--since" => Some((&mut a.since, "--since takes a rev")),
            "--wait" => Some((&mut a.wait, "--wait takes seconds")),
            "--agent" => Some((&mut a.agent, "--agent takes a handle")),
            "--project" => Some((&mut a.project, "--project takes a name")),
            "--level" => Some((&mut a.level, "--level takes read or write")),
            _ => None,
        };
        if let Some((slot, why)) = slot {
            *slot = val(why)?;
            if w == "--value" {
                a.has_value = true;
            }
            i += 2;
            continue;
        }
        match w {
            "--all" => a.all = true,
            "--follow" => a.follow = true,
            "--yes" | "-y" => a.yes = true,
            "--json" => *json = true,
            _ if w.starts_with("--") => return Err(format!("unknown flag {w}")),
            _ if a.pos.is_empty() => a.pos = w.to_string(),
            _ if a.pos2.is_empty() => a.pos2 = w.to_string(),
            _ => return Err(format!("unexpected '{w}'")),
        }
        i += 1;
    }
    let nums = format!("{}{}{}{}", a.ifrev, a.limit, a.since, a.wait);
    if !nums.bytes().all(|b| b.is_ascii_digit()) {
        return Err("--if-rev, --limit, --since and --wait take whole numbers".into());
    }
    Ok(a)
}

/// `_gd_body KIND [dry]`: the JSON body, or what its Python printed when
/// it stopped (the command ends with 2 either way).
pub fn body(kind: &str, widget: &str, a: &Args, dry: bool) -> Result<String, String> {
    let die = |m: String| format!("Error: {m}\n");
    let mut b = Dict::default();
    b.set("widget", s(widget));
    if matches!(kind, "put" | "patch" | "rm") {
        if a.pos.is_empty() {
            return Err(die(format!(
                concat!(
                    "a key is required (",
                    cli_name!(),
                    " store {} --widget <w> <key>)"
                ),
                kind
            )));
        }
        b.set("key", s(&a.pos));
        if !a.ifrev.is_empty() {
            b.set("ifRev", int(&a.ifrev));
        }
    }
    if matches!(kind, "put" | "patch") {
        let text = if !a.file.is_empty() {
            match pyjson::read_text_file(&a.file) {
                Ok(t) => t,
                Err(f) => return Err(Raise::at(17, f.last_line()).text()),
            }
        } else if a.has_value {
            a.value.clone()
        } else {
            return Err(die(format!("{kind} takes --value <json> or --file <path>")));
        };
        let value = match pyjson::loads(&text) {
            Ok(v) => v,
            Err(x) => return Err(die(format!("the value is not JSON: {}", x.message))),
        };
        if kind == "patch" && !matches!(value, Value::Object(_)) {
            return Err(die(
                "patch takes a JSON object of the fields to change (null removes a field)".into(),
            ));
        }
        b.set("value", value);
    }
    if matches!(kind, "grant" | "revoke" | "admin" | "unadmin") {
        if matches!(kind, "admin" | "unadmin") && !a.project.is_empty() {
            return Err(die("a grant admin is one agent (--agent)".into()));
        }
        if a.agent.is_empty() == a.project.is_empty() {
            return Err(die("name exactly one of --agent or --project".into()));
        }
        if !a.agent.is_empty() {
            b.set("agent", s(&a.agent));
        }
        if !a.project.is_empty() {
            b.set("project", s(&a.project));
        }
        b.set("prefix", s(&a.prefix));
        if matches!(kind, "grant" | "admin") {
            if !matches!(a.level.as_str(), "read" | "write") {
                return Err(die("--level is read or write".into()));
            }
            b.set("level", s(&a.level));
        }
        if kind == "unadmin" && dry {
            b.set("dryRun", Value::Bool(true));
        }
    }
    Ok(dumps_default(&b.value()))
}

/// `body="$(_gd_body …)" || exit 2`.
pub fn body_or_exit(kind: &str, widget: &str, a: &Args, dry: bool) -> R<String> {
    body(kind, widget, a, dry).map_err(|said| {
        tell(&said);
        Exit::Usage
    })
}

/// `Granted %s on %s to %s %s`.
pub fn granted(body: &[u8]) -> Read {
    lines(body, 1, |v, o, _| {
        let g = at(v, "grant", 1)?;
        let prefix = at(g, "prefix", 1)?;
        o.push_str(&format!(
            "Granted {} on {} to {} {}\n",
            py::str_of(at(g, "level", 1)?),
            if py::truthy(prefix) {
                py::str_of(prefix)
            } else {
                "everything".into()
            },
            py::str_of(at(g, "subjectKind", 1)?),
            py::str_of(at(g, "subjectId", 1)?)
        ));
        Ok(())
    })
}

/// `_gd_project_note`.
pub fn project_note(body: &[u8], project: &str) {
    if contains(body, b"project_grant_convenience") {
        tell(&format!("Note: any agent can join project \"{project}\", so this grant is a convenience, not a wall. For a wall, grant agents one by one or name a grant admin.\n"));
    }
}

/// `print("Revoked.")`, whatever came.
pub fn revoked(_: &[u8]) -> Read {
    Ok(Shown::out("Revoked.\n"))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `rev %s (%s ops)`.
fn rev_ops(body: &[u8]) -> Read {
    lines(body, 1, |v, o, _| {
        o.push_str(&format!(
            "rev {} ({} ops)\n",
            py::str_of(at(v, "rev", 1)?),
            py::str_of(at(v, "ops", 1)?)
        ));
        Ok(())
    })
}

/// `cmd_store_widget`: `words` are the words after `store`.
pub fn store(client: &mut Client, words: &[&str]) -> Exit {
    match store_run(client, words) {
        Ok(()) => Exit::Ok,
        Err(e) => e,
    }
}

fn store_run(client: &mut Client, words: &[&str]) -> R<()> {
    let sub = words.first().copied().unwrap_or("");
    let mut widget = String::new();
    let mut json = false;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 1;
    while i < words.len() {
        match words[i] {
            "--widget" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(usage("--widget takes a name"));
                };
                widget = v.to_string();
                i += 2;
                continue;
            }
            "--json" => json = true,
            w => rest.push(w.to_string()),
        }
        i += 1;
    }
    if widget.is_empty() {
        return Err(usage(concat!(
            "--widget takes a widget name (",
            cli_name!(),
            " zen widget list)"
        )));
    }
    let mut gd = Gd {
        client,
        json,
        widget,
    };
    match sub {
        "get" => get(&mut gd, &rest),
        "put" => put(&mut gd, &rest),
        "patch" => patch(&mut gd, &rest),
        "rm" | "delete" => rm(&mut gd, &rest),
        "list" | "ls" => list(&mut gd, &rest),
        "changes" => changes(&mut gd, &rest),
        "batch" => batch(&mut gd, &rest),
        "grants" => grants(&mut gd, &rest),
        "grant" => grant(&mut gd, &rest),
        "revoke" => revoke(&mut gd, &rest),
        "export" => export(&mut gd, &rest),
        "blob" => blob(&mut gd, &rest),
        "history" => history(&mut gd, &rest),
        "restore" => restore(&mut gd, &rest),
        "snapshot" => snapshot(&mut gd, &rest),
        _ => Err(usage(&format!(
            concat!(
                "unknown '",
                cli_name!(),
                " store {} --widget' command — see ",
                cli_name!(),
                " store --help"
            ),
            sub
        ))),
    }
}

/// `_gd_args "$@"` in a verb: the flags, or the usage exit.
pub fn args(gd: &mut Gd, words: &[String]) -> R<Args> {
    gd_args(words, &mut gd.json).map_err(|e| usage(&e))
}

fn get(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    if a.pos.is_empty() {
        return Err(usage(concat!(
            "usage: ",
            cli_name!(),
            " store get --widget <w> <key>"
        )));
    }
    let q = gd.q(&[("key", a.pos.as_str())]);
    let body = gd.call(false, "/cli/zen/data/get", &q)?;
    gd.out(&body, |b| {
        let v = load(b, 3).map_err(|r| (String::new(), String::new(), r))?;
        let e = at(&v, "entry", 3).map_err(|r| (String::new(), String::new(), r))?;
        if *e == Value::Null {
            return Ok(Shown::err("No such key.\n", Exit::NotFound));
        }
        let mut o = String::new();
        let value = at(e, "value", 7).map_err(|r| (String::new(), String::new(), r))?;
        o.push_str(&dumps_indented(value));
        o.push('\n');
        let rev = at(e, "rev", 8).map_err(|r| (o.clone(), String::new(), r))?;
        let by = at(e, "by", 8).map_err(|r| (o.clone(), String::new(), r))?;
        Ok(Shown {
            stdout: o,
            stderr: format!("rev {} by {}\n", py::str_of(rev), py::str_of(by)),
            exit: Exit::Ok,
        })
    })
}

fn put(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let body = body_or_exit("put", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/put", &body)?;
    gd.out(&ans, |b| {
        lines(b, 1, |v, o, _| {
            o.push_str(&format!("rev {}\n", py::str_of(at(v, "rev", 1)?)));
            Ok(())
        })
    })
}

fn patch(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let body = body_or_exit("patch", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/patch", &body)?;
    gd.out(&ans, |b| {
        lines(b, 3, |v, o, e| {
            o.push_str(&dumps_indented(at(v, "value", 4)?));
            o.push('\n');
            let changed = py::truthy(at(v, "changed", 5)?);
            let rev = py::str_of(at(v, "rev", 5)?);
            e.push_str(&if changed {
                format!("rev {rev}\n")
            } else {
                format!("rev {rev} (nothing changed)\n")
            });
            Ok(())
        })
    })
}

fn rm(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let body = body_or_exit("rm", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/delete", &body)?;
    gd.out(&ans, |b| {
        lines(b, 3, |v, o, _| {
            if py::truthy(at(v, "deleted", 4)?) {
                o.push_str(&format!("rev {}\n", py::str_of(at(v, "rev", 4)?)));
            } else {
                o.push_str("No such key (nothing changed).\n");
            }
            Ok(())
        })
    })
}

/// A list page's items, one `json.dumps(i, ensure_ascii=False)` line each.
fn item_lines(body: &[u8]) -> Result<Vec<Value>, Raise> {
    let v = load(body, 1)?;
    crate::families::py_iter(at(&v, "items", 1)?, 1)
}

fn list(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let mut after = a.after.clone();
    let mut all: Vec<Value> = Vec::new();
    let mut body;
    loop {
        let q = gd.q(&[
            ("prefix", a.prefix.as_str()),
            ("after", after.as_str()),
            ("limit", a.limit.as_str()),
        ]);
        body = gd.call(false, "/cli/zen/data/list", &q)?;
        if !a.all {
            break;
        }
        match item_lines(&body) {
            Ok(items) => all.extend(items),
            Err(r) => return Err(raise_out(&r)),
        }
        let next = load(&body, 1).and_then(|v| at(&v, "next", 1).cloned());
        match next {
            Ok(n) => {
                after = if py::truthy(&n) {
                    captured(&n)
                } else {
                    String::new()
                }
            }
            Err(r) => return Err(raise_out(&r)),
        }
        if after.is_empty() {
            break;
        }
    }
    if a.all {
        let mut page = Dict::default();
        // Each line went through json.dumps and back: the same value.
        page.set("items", Value::Array(all));
        page.set("next", Value::Null);
        body = dumps_text(&page.value()).into_bytes();
    }
    gd.out(&body, |b| {
        lines(b, 3, |v, o, e| {
            for i in crate::families::py_iter(at(v, "items", 4)?, 4)? {
                let value: String = dumps_text(at(&i, "value", 5)?).chars().take(200).collect();
                o.push_str(&format!(
                    "{}\trev {}\t{}\n",
                    py::str_of(at(&i, "key", 5)?),
                    py::str_of(at(&i, "rev", 5)?),
                    value
                ));
            }
            if let Some(n) = crate::families::py_get(v, "next", 6)? {
                if py::truthy(n) {
                    e.push_str(&format!("more: --after {}\n", py::str_of(n)));
                }
            }
            Ok(())
        })
    })
}

/// What one changes page did.
enum Page {
    Next {
        since: String,
        after: String,
        more: bool,
    },
    Resync,
}

fn raise_out(r: &Raise) -> Exit {
    match shown(raised(String::new(), String::new(), r)) {
        Ok(()) => Exit::Error,
        Err(e) => e,
    }
}

/// `_gd_changes_page`'s reader: each change's line, the next cursor and
/// whether there is more.
fn page_lines(body: &[u8], since: &str, after: &str) -> Result<PageRead, Raise> {
    let v = load(body, 3)?;
    let items = crate::families::py_iter(at(&v, "items", 4)?, 4)?;
    let lines: Vec<String> = items.iter().map(dumps_text).collect();
    let n = crate::families::py_get(&v, "next", 6)?;
    let more = n.is_some_and(py::truthy);
    let (since2, after2) = match n {
        Some(n) if py::truthy(n) => {
            let s2 = py::str_of(at(n, "since", 8)?);
            let a2 = match crate::families::py_get(n, "after", 8)? {
                Some(x) if py::truthy(x) => py::str_of(x),
                _ => String::new(),
            };
            (s2, a2)
        }
        _ => match items.last() {
            Some(last) => (py::str_of(at(last, "rev", 10)?), String::new()),
            None => (since.to_string(), after.to_string()),
        },
    };
    Ok((lines, since2, after2, more))
}

/// A changes page read: lines, since, after, more.
type PageRead = (Vec<String>, String, String, bool);

/// `_gd_changes_page SINCE AFTER WAIT`.
fn changes_page(gd: &mut Gd, a: &Args, since: &str, after: &str, wait: &str) -> R<Page> {
    let q = gd.q(&[
        ("since", since),
        ("after", after),
        ("prefix", a.prefix.as_str()),
        ("wait", wait),
    ]);
    let sent = gd.send(false, "/cli/zen/data/changes", &q)?;
    if sent.status == "409" && contains(&sent.resp, b"\"resync\"") {
        // `_GD_NEXT_SINCE="$(…)"` under `set -e`: a reader that raises
        // ends the command; else `return 9` does.
        let v = load(&sent.resp, 1).and_then(|v| {
            let e = at(&v, "error", 1)?;
            crate::families::py_get(e, "reapedRev", 1).map(|_| ())
        });
        if let Err(r) = v {
            return Err(raise_out(&r));
        }
        return Ok(Page::Resync);
    }
    if sent.status != "200" {
        return Err(gd.fail(&sent.status, &sent.resp));
    }
    // `out="$(…)"`: a reader that raises prints only its traceback, and
    // `set -e` ends the command.
    let (lines, since2, after2, more) = match page_lines(&sent.resp, since, after) {
        Ok(x) => x,
        Err(r) => return Err(raise_out(&r)),
    };
    if lines.is_empty() {
        // `printf … | grep -v '^#'` finds nothing: `pipefail` and `set -e`.
        return Err(Exit::Error);
    }
    let mut text = String::new();
    for l in &lines {
        text.push_str(l);
        text.push('\n');
    }
    say(text.as_bytes())?;
    // `${nl%% *}` and `${nl#* }` of the `#NEXT <since> <after>` line.
    let nl = format!("{since2} {after2}");
    let (head, rest) = nl.split_once(' ').unwrap_or((nl.as_str(), ""));
    Ok(Page::Next {
        since: head.to_string(),
        after: rest.to_string(),
        more,
    })
}

fn changes(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let mut since = if a.since.is_empty() {
        "0".to_string()
    } else {
        a.since.clone()
    };
    let mut after = String::new();
    let mut wait = if a.wait.is_empty() {
        "0".to_string()
    } else {
        a.wait.clone()
    };
    if gd.json && !a.follow {
        let q = gd.q(&[
            ("since", since.as_str()),
            ("prefix", a.prefix.as_str()),
            ("wait", wait.as_str()),
        ]);
        let body = gd.call(false, "/cli/zen/data/changes", &q)?;
        let mut b = body;
        b.push(b'\n');
        return say(&b);
    }
    if a.follow {
        wait = "30".into();
    }
    loop {
        match changes_page(gd, &a, &since, &after, &wait)? {
            // `_gd_changes_page …; rc=$?` under `set -e`.
            Page::Resync => return Err(Exit::from_curl(9)),
            Page::Next {
                since: s,
                after: n,
                more,
            } => {
                since = s;
                after = n;
                if more {
                    continue;
                }
                if !a.follow {
                    return Ok(());
                }
            }
        }
    }
}

/// `cmd_store_widget_batch`'s plan: each part's ops, compact, or what
/// its Python printed (the command ends with 2).
pub fn batch_plan(stdin: &str) -> Result<Vec<String>, String> {
    let raw: &str = stdin.trim_matches(crate::families::py_space);
    if raw.is_empty() {
        return Err(concat!(
            "Error: ",
            cli_name!(),
            " store batch reads ops on stdin (a JSON array or one op per line)\n"
        )
        .to_string());
    }
    let parsed = if raw.starts_with('[') {
        pyjson::loads(raw)
    } else {
        ops::splitlines(raw)
            .iter()
            .filter(|l| !l.trim_matches(crate::families::py_space).is_empty())
            .map(|l| pyjson::loads(l))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    };
    let ops_v = match parsed {
        Ok(v) => v,
        Err(e) => return Err(format!("Error: the ops are not JSON: {}\n", e.message)),
    };
    let Value::Array(ops_list) = ops_v else {
        return Err("Error: no ops\n".into());
    };
    if ops_list.is_empty() {
        return Err("Error: no ops\n".into());
    }
    if ops_list.len() > 1000 {
        return Err("Error: at most 1,000 ops in one batch\n".into());
    }
    let mut parts: Vec<Vec<Value>> = Vec::new();
    let mut cur: Vec<Value> = Vec::new();
    let mut size = 0usize;
    for op in ops_list {
        let has = ops::contains(&op, "value", 19).map_err(|r| r.text())?;
        let b = if has {
            let v = crate::families::py_get(&op, "value", 19).map_err(|r| r.text())?;
            dumps_compact(v.unwrap_or(&Value::Null), true)
                .chars()
                .count()
        } else {
            0
        };
        if !cur.is_empty() && (cur.len() == 64 || size + b > 60 * 1024) {
            parts.push(std::mem::take(&mut cur));
            size = 0;
        }
        cur.push(op);
        size += b;
    }
    parts.push(cur);
    Ok(parts
        .into_iter()
        .map(|p| dumps_compact(&Value::Array(p), true))
        .collect())
}

fn batch(gd: &mut Gd, words: &[String]) -> R<()> {
    let _ = args(gd, words)?;
    let mut input = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut input);
    let plan = batch_plan(&String::from_utf8_lossy(&input)).map_err(|said| {
        tell(&said);
        Exit::Usage
    })?;
    let wjson = dumps_default(&s(&gd.widget));
    if plan.len() == 1 {
        let ans = gd.call(
            true,
            "/cli/zen/data/batch",
            &format!("{{\"widget\":{wjson},\"ops\":{}}}", plan[0]),
        )?;
        return gd.out(&ans, rev_ops);
    }
    let begun = gd.call(
        true,
        "/cli/zen/data/tx/begin",
        &format!("{{\"widget\":{wjson}}}"),
    )?;
    let tx = match load(&begun, 1).and_then(|v| at(&v, "txId", 1).map(captured)) {
        Ok(t) => t,
        Err(r) => return Err(raise_out(&r)),
    };
    for part in &plan {
        let sent = gd.send(
            true,
            "/cli/zen/data/tx/add",
            &format!("{{\"txId\":\"{tx}\",\"ops\":{part}}}"),
        )?;
        if sent.status != "200" {
            // Soft: whatever the abort meets, nothing is printed.
            let _ = gd.client.compute_send(
                true,
                "/cli/zen/data/tx/abort",
                &format!("{{\"txId\":\"{tx}\"}}"),
                false,
            );
            return Err(gd.fail(&sent.status, &sent.resp));
        }
    }
    let ans = gd.call(
        true,
        "/cli/zen/data/tx/commit",
        &format!("{{\"txId\":\"{tx}\"}}"),
    )?;
    gd.out(&ans, rev_ops)
}

fn grants(gd: &mut Gd, words: &[String]) -> R<()> {
    let _ = args(gd, words)?;
    let q = gd.q(&[]);
    let body = gd.call(false, "/cli/zen/data/grants", &q)?;
    gd.out(&body, |b| {
        lines(b, 3, |v, o, _| {
            let gs = at(v, "grants", 4)?;
            if !py::truthy(gs) {
                o.push_str(&format!(
                    concat!(
                        "No access to {}. Ask the person (",
                        cli_name!(),
                        " feedback or the Thread).\n"
                    ),
                    py::str_of(at(v, "space", 5)?)
                ));
            }
            for g in crate::families::py_iter(gs, 6)? {
                let from = crate::families::py_get(&g, "from", 7)?;
                let f = match from {
                    Some(x) if py::truthy(x) => x.clone(),
                    _ => Value::Object(Vec::new()),
                };
                let name = crate::families::py_get(&f, "name", 8)?
                    .cloned()
                    .unwrap_or_else(|| s("?"));
                let kind = crate::families::py_get(&f, "kind", 8)?;
                let mut src = match kind {
                    Some(Value::Str(t)) if t.as_str() == Some("agent") => "you".to_string(),
                    Some(Value::Str(t)) if t.as_str() == Some("delegation") => {
                        "grant admin".to_string()
                    }
                    _ => format!("project {}", py::str_of(&name)),
                };
                if !ops::contains(&g, "from", 9)? {
                    src = format!(
                        "{} {}",
                        py::str_of(at(&g, "subjectKind", 10)?),
                        py::str_of(at(&g, "subjectId", 10)?)
                    );
                }
                let prefix = at(&g, "prefix", 11)?;
                let prefix = if py::truthy(prefix) {
                    py::str_of(prefix)
                } else {
                    "(everything)".into()
                };
                let level = at(&g, "level", 11)?;
                let admin = crate::families::py_get(&g, "admin", 11)?.is_some_and(py::truthy);
                o.push_str(&format!(
                    "{} {} {}{}\n",
                    crate::families::ljust(&prefix, 24),
                    left(level, 5),
                    src,
                    if admin { " (admin)" } else { "" }
                ));
            }
            let made = crate::families::py_get(v, "made", 12)?;
            if let Some(m) = made.filter(|m| py::truthy(m)) {
                for g in crate::families::py_iter(m, 12)? {
                    let prefix = at(&g, "prefix", 13)?;
                    let prefix = if py::truthy(prefix) {
                        py::str_of(prefix)
                    } else {
                        "(everything)".into()
                    };
                    o.push_str(&format!(
                        "made: {} {} -> {} {}\n",
                        crate::families::ljust(&prefix, 18),
                        left(at(&g, "level", 13)?, 5),
                        py::str_of(at(&g, "subjectKind", 13)?),
                        py::str_of(at(&g, "subjectId", 13)?)
                    ));
                }
            }
            Ok(())
        })
    })
}

fn grant(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let body = body_or_exit("grant", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/grant", &body)?;
    project_note(&ans, &a.project);
    gd.out(&ans, granted)
}

fn revoke(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    let body = body_or_exit("revoke", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/revoke", &body)?;
    gd.out(&ans, revoked)
}

/// `_gd_take_at USAGE ARGS…`: `--at`'s date, or the usage exit.
pub fn take_at(usage_line: &str, words: &[String]) -> R<String> {
    let mut at_ = String::new();
    let mut i = 0;
    while i < words.len() {
        match words[i].as_str() {
            "--at" => {
                at_ = words.get(i + 1).cloned().unwrap_or_default();
                if at_.is_empty() {
                    return Err(usage("--at takes a date like 2026-10-13"));
                }
                i += 2;
            }
            "--json" => i += 1,
            w => return Err(usage(&format!("unexpected '{w}' ({usage_line})"))),
        }
    }
    let b = at_.as_bytes();
    let dated = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !at_.is_empty() && !dated {
        return Err(usage(
            "--at takes a date like 2026-10-13 (YYYY-MM-DD, this computer's time zone)",
        ));
    }
    Ok(at_)
}

/// `mktemp "${TMPDIR:-/tmp}/k2-gd-next.XXXXXX"`: made, and taken away
/// again (the binary keeps the cursor itself); the shell's line when it
/// can't be made.
fn temp_check() -> R<()> {
    let dir = std::env::var_os("TMPDIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into());
    match make_temp(&dir.join("k2-gd-next.")) {
        Some((path, _)) => {
            let _ = std::fs::remove_file(path);
            Ok(())
        }
        None => {
            tell("Error: can't make a temp file\n");
            Err(Exit::Error)
        }
    }
}

/// A new file named `<stem>` and six random letters, 0600.
pub fn make_temp(stem: &std::path::Path) -> Option<(std::path::PathBuf, std::fs::File)> {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    for _ in 0..100 {
        let tail: String = board::random_hex(6)
            .as_bytes()
            .chunks(2)
            .map(|c| {
                let n = u8::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 16).unwrap_or(0);
                ALPHA[usize::from(n) % ALPHA.len()] as char
            })
            .collect();
        let mut name = stem.as_os_str().to_os_string();
        name.push(&tail);
        let path = std::path::PathBuf::from(name);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&path) {
            Ok(f) => return Some((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// `_gd_export_jsonl WIDGET AT`: every page, as JSONL on stdout, the
/// header first.
pub fn export_jsonl(gd: &mut Gd, widget: &str, at_: &str) -> R<()> {
    export_jsonl_to(gd, widget, at_, &mut say)
}

/// [`export_jsonl`], each page's lines handed to `sink`.
pub fn export_jsonl_to(
    gd: &mut Gd,
    widget: &str,
    at_: &str,
    sink: &mut dyn FnMut(&[u8]) -> R<()>,
) -> R<()> {
    temp_check()?;
    let mut after = String::new();
    let mut first = true;
    loop {
        let pairs: Vec<(&str, &str)> = [("widget", widget), ("at", at_), ("after", after.as_str())]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let q = urlencode(&pairs);
        let body = gd.call(false, "/cli/zen/data/export", &q)?;
        let mut o = String::new();
        let read = (|| -> Result<String, Raise> {
            let page = load(&body, 4)?;
            if first {
                o.push_str(&dumps_compact(at(&page, "header", 7)?, false));
                o.push('\n');
            }
            for it in crate::families::py_iter(at(&page, "items", 8)?, 8)? {
                let mut row = Dict::default();
                for k in ["key", "rev", "value", "updatedAt", "by"] {
                    row.set(k, at(&it, k, 9)?.clone());
                }
                o.push_str(&dumps_compact(&row.value(), false));
                o.push('\n');
            }
            let next = crate::families::py_get(&page, "next", 13)?;
            match next {
                Some(n) if py::truthy(n) => match n {
                    Value::Str(_) => Ok(py::str_of(n)),
                    other => Err(Raise::at(
                        13,
                        format!(
                            "TypeError: write() argument must be str, not {}",
                            py::type_name(other)
                        ),
                    )),
                },
                _ => Ok(String::new()),
            }
        })();
        sink(o.as_bytes())?;
        match read {
            Ok(n) => after = n.trim_end_matches('\n').to_string(),
            Err(r) => {
                tell(&r.text());
                tell("Error: the export page wasn't readable JSON\n");
                return Err(Exit::Error);
            }
        }
        first = false;
        if after.is_empty() {
            return Ok(());
        }
    }
}

fn export(gd: &mut Gd, words: &[String]) -> R<()> {
    let at_ = take_at(
        concat!(
            cli_name!(),
            " store export --widget <w> [--at YYYY-MM-DD] > data.jsonl"
        ),
        words,
    )?;
    let w = gd.widget.clone();
    export_jsonl(gd, &w, &at_)
}

/// `base64.b64encode(b).decode()`.
pub fn b64encode(b: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        s.push(ALPHA[(n >> 18) as usize & 63] as char);
        s.push(ALPHA[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            ALPHA[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            ALPHA[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

#[cfg(unix)]
fn readable(p: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(std::ffi::OsStr::new(p).as_bytes()) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
}

#[cfg(not(unix))]
fn readable(p: &str) -> bool {
    std::fs::File::open(p).is_ok()
}

/// `basename "$file"`.
fn basename(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() {
            String::new()
        } else {
            "/".into()
        };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// `_gd_blob_put_file WIDGET FILE MIME NAME`: the last answer's body.
pub fn blob_put_file(gd: &mut Gd, widget: &str, file: &str, mime: &str, name: &str) -> R<Vec<u8>> {
    let meta = std::fs::metadata(file);
    if !(meta.as_ref().is_ok_and(|m| m.is_file()) && readable(file)) {
        return Err(usage(&format!("can't read '{file}'")));
    }
    let size = meta.map(|m| m.len()).unwrap_or(0);
    if size > 16_777_216 {
        tell(&format!(
            "Error: too_large: a file is at most 16 MiB; '{file}' is {size} bytes\n"
        ));
        return Err(Exit::Error);
    }
    let name = if name.is_empty() {
        basename(file)
    } else {
        name.to_string()
    };
    let mut offset = 0u64;
    let mut upload = String::new();
    loop {
        let chunk = (|| -> std::io::Result<Vec<u8>> {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = std::fs::File::open(file)?;
            f.seek(SeekFrom::Start(offset))?;
            let mut buf = Vec::new();
            f.take(BLOB_CHUNK).read_to_end(&mut buf)?;
            Ok(buf)
        })();
        let Ok(chunk) = chunk else {
            tell(&format!("Error: can't read '{file}'\n"));
            return Err(Exit::Error);
        };
        let last = offset + chunk.len() as u64 >= size;
        let mut b = Dict::default();
        b.set("widget", s(widget));
        b.set("offset", Value::Int(offset.to_string()));
        b.set("base64", s(&b64encode(&chunk)));
        b.set("last", Value::Bool(last));
        if !upload.is_empty() {
            b.set("upload", s(&upload));
        }
        if !mime.is_empty() {
            b.set("mime", s(mime));
        }
        if !name.is_empty() {
            b.set("name", s(&name));
        }
        let ans = gd.call(true, "/cli/zen/data/blob/put", &dumps_default(&b.value()))?;
        if last {
            return Ok(ans);
        }
        upload = match load(&ans, 1).and_then(|v| at(&v, "upload", 1).map(captured)) {
            Ok(u) => u,
            Err(r) => return Err(raise_out(&r)),
        };
        offset += BLOB_CHUNK;
    }
}

/// `_gd_blob_get_file WIDGET REF OUT PX`: writes OUT whole (or not at
/// all); the last answer without its bytes.
pub fn blob_get_file(gd: &mut Gd, widget: &str, rf: &str, out: &str, px: &str) -> R<Vec<u8>> {
    let Some((tmp, mut f)) = make_temp(std::path::Path::new(&format!("{out}.k2part."))) else {
        tell(&format!("Error: can't write next to '{out}'\n"));
        return Err(Exit::Error);
    };
    let gone = |e: Exit| {
        let _ = std::fs::remove_file(&tmp);
        e
    };
    let mut offset: i128 = 0;
    loop {
        let off = offset.to_string();
        let chunk = BLOB_CHUNK.to_string();
        let mut pairs: Vec<(&str, &str)> = vec![
            ("widget", widget),
            ("ref", rf),
            ("offset", off.as_str()),
            ("len", chunk.as_str()),
        ];
        if !px.is_empty() {
            pairs.push(("px", px));
        }
        let body = gd
            .call(false, "/cli/zen/data/blob/get", &urlencode(&pairs))
            .map_err(gone)?;
        let read = (|| -> Result<(bool, String, Value), Raise> {
            let v = load(&body, 4)?;
            let b64 = at(&v, "base64", 6)?;
            let bytes = match b64 {
                Value::Str(_) => crate::families::compute::writes::fetch::b64decode(&py::str_of(b64))
                    .map_err(|e| Raise::at(6, e))?,
                other => {
                    return Err(Raise::at(
                        6,
                        format!(
                            "TypeError: argument should be a bytes-like object or ASCII string, not '{}'",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            f.write_all(&bytes)
                .map_err(|e| Raise::at(6, pyjson::os_error(&e, "").last_line()))?;
            let eof = py::truthy(at(&v, "eof", 7)?);
            let len = py::str_of(at(&v, "len", 7)?);
            Ok((eof, len, v))
        })();
        let (eof, len, v) = match read {
            Ok(x) => x,
            Err(r) => {
                tell(&r.text());
                tell("Error: the daemon's answer wasn't readable\n");
                return Err(gone(Exit::Error));
            }
        };
        let n: i128 = len.trim().parse().unwrap_or(0);
        offset += n;
        if eof {
            drop(f);
            if std::fs::rename(&tmp, out).is_err() {
                tell(&format!("Error: can't write '{out}'\n"));
                return Err(gone(Exit::Error));
            }
            let mut rest = Dict::default();
            if let Value::Object(pairs) = v {
                for (k, x) in pairs {
                    if !matches!(k.as_str(), Some("base64" | "offset" | "len" | "eof")) {
                        rest.0.push((k, x));
                    }
                }
            }
            return Ok(dumps_default(&rest.value()).into_bytes());
        }
        if n <= 0 {
            tell("Error: the daemon stopped before the end\n");
            return Err(gone(Exit::Error));
        }
    }
}

fn blob(gd: &mut Gd, words: &[String]) -> R<()> {
    let sub = words.first().map(String::as_str).unwrap_or("");
    let (mut pos, mut mime, mut name, mut out, mut px, mut after, mut limit) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let w = if words.is_empty() {
        &[][..]
    } else {
        &words[1..]
    };
    let mut i = 0;
    while i < w.len() {
        let a = w[i].as_str();
        let slot = match a {
            "--mime" => Some((&mut mime, "--mime takes a type")),
            "--name" => Some((&mut name, "--name takes a file name")),
            "--out" | "-o" => Some((&mut out, "--out takes a path")),
            "--px" => Some((&mut px, "--px takes 256 or 512")),
            "--after" => Some((&mut after, "--after takes a ref")),
            "--limit" => Some((&mut limit, "--limit takes a number")),
            _ => None,
        };
        if let Some((slot, why)) = slot {
            let Some(v) = w.get(i + 1) else {
                return Err(usage(why));
            };
            *slot = v.clone();
            i += 2;
            continue;
        }
        match a {
            "--json" => gd.json = true,
            _ if a.starts_with("--") => return Err(usage(&format!("unknown flag {a}"))),
            _ if pos.is_empty() => pos = a.to_string(),
            _ => return Err(usage(&format!("unexpected '{a}'"))),
        }
        i += 1;
    }
    if !matches!(px.as_str(), "" | "256" | "512") {
        return Err(usage("--px is 256 or 512"));
    }
    if !limit.bytes().all(|b| b.is_ascii_digit()) {
        return Err(usage("--limit takes a whole number"));
    }
    let widget = gd.widget.clone();
    match sub {
        "put" => {
            if pos.is_empty() {
                return Err(usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " store blob put <file> --widget <w> [--mime m] [--name n]"
                )));
            }
            let ans = blob_put_file(gd, &widget, &pos, &mime, &name)?;
            gd.out(&ans, |b| {
                lines(b, 1, |v, o, _| {
                    o.push_str(&format!("{}\n", py::str_of(at(v, "ref", 1)?)));
                    Ok(())
                })
            })
        }
        "get" => {
            if pos.is_empty() || out.is_empty() {
                return Err(usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " store blob get <ref> --widget <w> --out <file> [--px 256|512]"
                )));
            }
            let ans = blob_get_file(gd, &widget, &pos, &out, &px)?;
            gd.out(&ans, |b| {
                lines(b, 1, |v, o, _| {
                    o.push_str(&format!(
                        "{} bytes, {}\n",
                        py::str_of(at(v, "size", 1)?),
                        py::str_of(at(v, "mime", 1)?)
                    ));
                    Ok(())
                })
            })
        }
        "ls" | "list" => {
            let q = gd.q(&[("after", after.as_str()), ("limit", limit.as_str())]);
            let body = gd.call(false, "/cli/zen/data/blob/list", &q)?;
            gd.out(&body, |b| {
                lines(b, 3, |v, o, e| {
                    for bl in crate::families::py_iter(at(v, "items", 4)?, 4)? {
                        let flag = if py::truthy(at(&bl, "pending", 5)?) {
                            "pending".to_string()
                        } else {
                            let refs = at(&bl, "refs", 5)?;
                            let one = matches!(refs, Value::Int(n) if n == "1")
                                || matches!(refs, Value::Bool(true));
                            format!("{} key{}", d(refs, 5)?, if one { "" } else { "s" })
                        };
                        let size = d(at(&bl, "size", 6)?, 6)?;
                        let name = at(&bl, "name", 6)?;
                        o.push_str(&format!(
                            "{}  {:>9}  {} {} {}\n",
                            py::str_of(at(&bl, "ref", 6)?),
                            size,
                            left(at(&bl, "mime", 6)?, 24),
                            crate::families::ljust(&flag, 10),
                            if py::truthy(name) {
                                py::str_of(name)
                            } else {
                                String::new()
                            }
                        ));
                    }
                    e.push_str(&format!(
                        "{} of {} MiB used\n",
                        floor_mib(at(v, "bytes", 7)?, 7)?,
                        floor_mib(at(v, "quota", 7)?, 7)?
                    ));
                    let next = at(v, "next", 8)?;
                    if py::truthy(next) {
                        e.push_str(&format!("more: --after {}\n", py::str_of(next)));
                    }
                    Ok(())
                })
            })
        }
        _ => Err(usage(concat!(
            cli_name!(),
            " store blob put|get|ls --widget <w> (see ",
            cli_name!(),
            " store --help)"
        ))),
    }
}

/// `'%d' % (n // 1048576)`.
fn floor_mib(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Int(digits) => match digits.parse::<i128>() {
            Ok(n) => Ok(n.div_euclid(1_048_576).to_string()),
            Err(_) => Ok(String::from("0")),
        },
        Value::Bool(_) => Ok("0".into()),
        Value::Float(f) => d(&Value::Float((f / 1_048_576.0).floor()), line),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for //: '{}' and 'int'",
                py::type_name(other)
            ),
        )),
    }
}

/// `_gd_restore_body KIND WIDGET ARGS…`: the JSON body, or what its
/// Python printed (the command ends with 2).
pub fn restore_body(kind: &str, widget: &str, args: &[String]) -> Result<String, String> {
    let die = |m: String| format!("Error: {m}\n");
    let mut b = Dict::default();
    b.set("widget", s(widget));
    let (mut pre, mut exc): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let val = || -> Result<String, String> {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| die(format!("{a} takes a value")))
        };
        match a {
            "--prefix" => {
                pre.push(s(&val()?));
                i += 2;
            }
            "--except" => {
                exc.push(s(&val()?));
                i += 2;
            }
            "--label" if kind == "snapshot" => {
                b.set("label", s(&val()?));
                i += 2;
            }
            "--rev" if kind == "restore" => {
                let v = val()?;
                if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(die(concat!(
                        "--rev takes a rev (a whole number; see ",
                        cli_name!(),
                        " store history)"
                    )
                    .to_string()));
                }
                b.set("toRev", int(&v));
                i += 2;
            }
            "--at" if kind == "restore" => {
                let v = val()?;
                if !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()) {
                    b.set("at", int(&v));
                } else if local_stamp(&v) {
                    b.set("atLocal", s(&v));
                } else {
                    return Err(die(
                        "--at takes 2026-10-26T17:30 (local time) or seconds since 1970".into(),
                    ));
                }
                i += 2;
            }
            "--snapshot" if kind == "restore" => {
                b.set("snapshot", s(&val()?));
                i += 2;
            }
            "--dry-run" if kind == "restore" => {
                b.set("dryRun", Value::Bool(true));
                i += 1;
            }
            _ => {
                return Err(die(format!(
                    concat!("unexpected {} (see ", cli_name!(), " store --help)"),
                    a
                )))
            }
        }
    }
    if kind == "restore"
        && ["toRev", "at", "atLocal", "snapshot"]
            .iter()
            .filter(|k| b.has(k))
            .count()
            != 1
    {
        return Err(die("name exactly one of --rev, --at or --snapshot".into()));
    }
    if !pre.is_empty() {
        b.set("prefixes", Value::Array(pre));
    }
    if !exc.is_empty() {
        b.set("except", Value::Array(exc));
    }
    Ok(dumps_default(&b.value()))
}

/// `re.match(r"^\d{4}-\d\d-\d\dT\d\d:\d\d(:\d\d)?$", v)`: `$` also
/// matches before one last newline.
fn local_stamp(v: &str) -> bool {
    let v = v.strip_suffix('\n').unwrap_or(v);
    let b = v.as_bytes();
    let shape: &[u8] = if b.len() == 19 {
        b"dddd-dd-ddTdd:dd:dd"
    } else {
        b"dddd-dd-ddTdd:dd"
    };
    b.len() == shape.len()
        && b.iter().zip(shape).all(|(c, p)| match p {
            b'd' => c.is_ascii_digit(),
            p => c == p,
        })
}

/// `_GD_RESTORE_OUT`.
pub fn restore_out(body: &[u8]) -> Read {
    lines(body, 3, |v, o, e| {
        let fam = ops::join(at(v, "families", 4)?, ", ", 4)?;
        let fam = if fam.is_empty() {
            "nothing".to_string()
        } else {
            fam
        };
        let id = at(v, "id", 5)?;
        let rev = || at(v, "rev", 5);
        let would = *id == Value::Null
            && *rev()? == Value::Null
            && (py::truthy(at(v, "written", 5)?) || py::truthy(at(v, "deleted", 5)?));
        if would {
            o.push_str(&format!(
                "Would write {} and delete {} key(s) in {}:\n",
                d(at(v, "written", 6)?, 6)?,
                d(at(v, "deleted", 6)?, 6)?,
                fam
            ));
            for k in crate::families::py_iter(at(v, "keys", 7)?, 7)? {
                match &k {
                    Value::Str(_) => o.push_str(&format!("  {}\n", py::str_of(&k))),
                    other => {
                        return Err(Raise::at(
                            8,
                            format!(
                                "TypeError: can only concatenate str (not \"{}\") to str",
                                py::type_name(other)
                            ),
                        ))
                    }
                }
            }
        } else if *at(v, "rev", 9)? == Value::Null {
            o.push_str("Nothing to change.\n");
        } else {
            let rev = at(v, "rev", 12)?;
            o.push_str(&format!(
                "Restored: wrote {}, deleted {} ({}) as rev {}, restore {}.\n",
                d(at(v, "written", 12)?, 12)?,
                d(at(v, "deleted", 12)?, 12)?,
                fam,
                d(rev, 12)?,
                py::str_of(at(v, "id", 12)?)
            ));
            let before = match rev {
                Value::Int(n) => match n.parse::<i128>() {
                    Ok(n) => Value::Int((n - 1).to_string()),
                    Err(_) => rev.clone(),
                },
                Value::Bool(b) => Value::Int(if *b { "0".into() } else { "-1".into() }),
                Value::Float(f) => Value::Float(f - 1.0),
                other => {
                    return Err(Raise::at(
                        13,
                        format!(
                            "TypeError: unsupported operand type(s) for -: '{}' and 'int'",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            e.push_str(&format!("Undo it with --rev {}.\n", d(&before, 13)?));
        }
        Ok(())
    })
}

fn history(gd: &mut Gd, words: &[String]) -> R<()> {
    let (mut key, mut prefix, mut limit, mut before) =
        (String::new(), String::new(), String::new(), String::new());
    let mut all = false;
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        let slot = match a {
            "--key" => Some((&mut key, "--key takes a key")),
            "--prefix" => Some((&mut prefix, "--prefix takes a prefix")),
            "--limit" => Some((&mut limit, "--limit takes a number")),
            "--before" => Some((&mut before, "--before takes a rev")),
            _ => None,
        };
        if let Some((slot, why)) = slot {
            let Some(v) = words.get(i + 1) else {
                return Err(usage(why));
            };
            *slot = v.clone();
            i += 2;
            continue;
        }
        match a {
            "--all" => all = true,
            "--json" => gd.json = true,
            _ => {
                return Err(usage(&format!(
                    concat!(
                        "unexpected '{}' (",
                        cli_name!(),
                        " store history --widget <w> [--key k | --prefix p/] [--limit N] [--all])"
                    ),
                    a
                )))
            }
        }
        i += 1;
    }
    if !format!("{limit}{before}")
        .bytes()
        .all(|b| b.is_ascii_digit())
    {
        return Err(usage("--limit and --before take whole numbers"));
    }
    let mut after = String::new();
    let mut body;
    loop {
        let q = gd.q(&[
            ("key", key.as_str()),
            ("prefix", prefix.as_str()),
            ("limit", limit.as_str()),
            ("before", before.as_str()),
            ("after", after.as_str()),
        ]);
        body = gd.call(false, "/cli/zen/data/history", &q)?;
        gd.out(&body, |b| {
            lines(b, 3, |v, o, _| {
                for it in crate::families::py_iter(at(v, "items", 4)?, 4)? {
                    let what = if py::truthy(at(&it, "deleted", 5)?) {
                        "deleted"
                    } else if ops::py_eq(at(&it, "prevKind", 5)?, &s("none")) {
                        "made"
                    } else {
                        "changed"
                    };
                    let mark = match crate::families::py_get(&it, "restore", 6)? {
                        Some(r) if py::truthy(r) => format!("  (restore {})", py::str_of(r)),
                        _ => String::new(),
                    };
                    let rev = d(at(&it, "rev", 7)?, 7)?;
                    o.push_str(&format!(
                        "rev {} {} {}  by {}{}\n",
                        crate::families::ljust(&rev, 6),
                        crate::families::ljust(what, 8),
                        py::str_of(at(&it, "key", 7)?),
                        py::str_of(at(&it, "by", 7)?),
                        mark
                    ));
                }
                Ok(())
            })
        })?;
        if !all {
            break;
        }
        let next = load(&body, 1).and_then(|v| at(&v, "next", 1).cloned());
        let next = match next {
            Ok(n) => n,
            Err(r) => return Err(raise_out(&r)),
        };
        let (b2, a2) = if py::truthy(&next) {
            let b2 = match at(&next, "before", 1) {
                Ok(x) => captured(x),
                Err(r) => return Err(raise_out(&r)),
            };
            let a2 = match crate::families::py_get(&next, "after", 1) {
                Ok(x) => x.map(captured).unwrap_or_default(),
                Err(r) => return Err(raise_out(&r)),
            };
            (b2, a2)
        } else {
            (String::new(), String::new())
        };
        before = b2;
        after = a2;
        if before.is_empty() {
            break;
        }
    }
    if gd.json {
        return Ok(());
    }
    reader(lines(&body, 3, |v, _, e| {
        e.push_str(&format!(
            "head rev {}; restorable back to rev {} (7 days of changes)\n",
            d(at(v, "head", 4)?, 4)?,
            d(at(v, "oldest", 4)?, 4)?
        ));
        let next = at(v, "next", 5)?;
        if py::truthy(next) {
            e.push_str(&format!(
                "more: --before {} (or --all)\n",
                d(at(next, "before", 6)?, 6)?
            ));
        }
        Ok(())
    }))
}

/// `--json` taken out; the rest passed on.
fn without_json(gd: &mut Gd, words: &[String]) -> Vec<String> {
    let mut pass = Vec::new();
    for w in words {
        if w == "--json" {
            gd.json = true;
        } else {
            pass.push(w.clone());
        }
    }
    pass
}

fn restore(gd: &mut Gd, words: &[String]) -> R<()> {
    let pass = without_json(gd, words);
    let body = restore_body("restore", &gd.widget, &pass).map_err(|said| {
        tell(&said);
        Exit::Usage
    })?;
    let ans = gd.call(true, "/cli/zen/data/restore", &body)?;
    gd.out(&ans, restore_out)
}

fn snapshot(gd: &mut Gd, words: &[String]) -> R<()> {
    let pass = without_json(gd, words);
    let body = restore_body("snapshot", &gd.widget, &pass).map_err(|said| {
        tell(&said);
        Exit::Usage
    })?;
    let ans = gd.call(true, "/cli/zen/data/snapshot", &body)?;
    gd.out(&ans, |b| {
        lines(b, 3, |v, o, e| {
            o.push_str(&format!("{}\n", py::str_of(at(v, "ref", 4)?)));
            e.push_str(&format!(
                "{} key(s) at rev {}, {} bytes. Keep the ref under a key (it goes after 24 h otherwise).\n",
                d(at(v, "keys", 5)?, 5)?,
                d(at(v, "rev", 5)?, 5)?,
                d(at(v, "bytes", 5)?, 5)?
            ));
            Ok(())
        })
    })
}
