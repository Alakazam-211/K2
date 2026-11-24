//! The sends waiting for a person's approval: `hostmail approvals`
//! (list, show, save, approve, deny), as `cmd_mail_approvals`
//! (`cli/src/mail-watch.sh`) reads the words and `_mail_py`'s
//! `approvals_*` verbs answer (`cli/src/mail.sh`).
//!
//! `hostmail approvals` alone, or with a flag first, is `list`. `save`
//! writes the file to a new path only (`O_EXCL`, `O_NOFOLLOW`, mode 0600)
//! and never opens it.

use crate::families::mail::core::{self, print, run_env, usage, At, Plan, Py, Secs, MODULE, R};
use crate::families::mail::py::{self as p, attr_get, iter, join, py_index};
use crate::families::mail::reads::b64decode;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::json as pyjson;
use k2_client::brand::CLI_NAME;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["approvals"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const HUMAN_SIZE: u32 = 284;
    pub const FILE_LINE: u32 = 292;
    pub const SHOWN_AS: u32 = 298;
    pub const LIST_CALL: u32 = 1522;
    pub const LIST_ROWS: u32 = 1523;
    pub const LIST_COUNT: u32 = 1529;
    pub const LIST_FOR: u32 = 1531;
    pub const LIST_WHO: u32 = 1532;
    pub const LIST_EXP_IF: u32 = 1534;
    pub const LIST_EXP: u32 = 1535;
    pub const LIST_HEAD: u32 = 1537;
    pub const LIST_TO: u32 = 1539;
    pub const LIST_CC_IF: u32 = 1540;
    pub const LIST_FILES: u32 = 1555;
    pub const LIST_FILE: u32 = 1556;
    pub const LIST_PREVIEW_IF: u32 = 1557;
    pub const LIST_OPEN: u32 = 1559;
    pub const LIST_DECIDE: u32 = 1560;
    pub const SHOW_CALL: u32 = 1565;
    pub const SHOW_MESSAGE: u32 = 1569;
    pub const SHOW_WHO: u32 = 1570;
    pub const SHOW_HEAD: u32 = 1571;
    pub const SHOW_TO: u32 = 1572;
    pub const SHOW_CC_IF: u32 = 1573;
    pub const SHOW_FILES: u32 = 1588;
    pub const SHOW_FILE: u32 = 1589;
    pub const SHOW_SAVE_IF: u32 = 1590;
    pub const SHOW_TEXT: u32 = 1593;
    pub const SAVE_CALL: u32 = 1601;
    pub const SAVE_DATA: u32 = 1603;
    pub const SAVE_JSON: u32 = 1611;
    pub const SAVE_PRINT: u32 = 1614;
    pub const APPROVE_CALL: u32 = 1622;
    pub const APPROVE_PRINT: u32 = 1626;
    pub const DENY_CALL: u32 = 1629;
    pub const DENY_PRINT: u32 = 1634;
}

/// The words after `hostmail approvals`, as `cmd_mail_approvals` reads
/// them.
pub fn parse(_verb: &str, words: &[&str]) -> Plan {
    if words.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return Plan::Help("hostmail/approvals");
    }
    let (sub, rest) = match words.first().copied() {
        Some(s @ ("list" | "show" | "save" | "approve" | "deny")) => (s, &words[1..]),
        None | Some("") => ("list", words),
        Some(w) if w.starts_with('-') => ("list", words),
        Some(w) => {
            return usage(format!(
                "unknown approvals command '{w}' — usage: {CLI_NAME} hostmail approvals [list | show <id> | save <id> --file <n> --out <path> | approve <id> [--note …] | deny <id> --note …]"
            ))
        }
    };
    let (mut id, mut note, mut file, mut out) = ("", "", "", "");
    let mut json = false;
    let mut i = 0;
    while let Some(w) = rest.get(i).copied() {
        let value = |missing: &str| rest.get(i + 1).copied().ok_or_else(|| usage(missing));
        match w {
            "--note" => match value("--note requires a value") {
                Ok(v) => note = v,
                Err(u) => return u,
            },
            "--file" => match value(&format!(
                "--file requires a file number (from {CLI_NAME} hostmail approvals show <id>)"
            )) {
                Ok(v) => file = v,
                Err(u) => return u,
            },
            "--out" => match value("--out requires a path to write the file to") {
                Ok(v) => out = v,
                Err(u) => return u,
            },
            "--json" => {
                json = true;
                i += 1;
                continue;
            }
            f if f.starts_with('-') => {
                return usage(format!(
                    "unknown flag '{f}' — see {CLI_NAME} hostmail approvals --help"
                ))
            }
            w => {
                if id.is_empty() {
                    id = w;
                } else {
                    return usage(format!("unexpected extra argument '{w}' — one <id> only"));
                }
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let need_id = |what: &str| {
        usage(format!(
            "{what} requires an <id> (out_… from '{CLI_NAME} hostmail approvals')"
        ))
    };
    match sub {
        "list" => {
            if !id.is_empty() || !note.is_empty() {
                return usage(format!(
                    "approvals list takes no <id>/--note — usage: {CLI_NAME} hostmail approvals [--json]"
                ));
            }
            Plan::Run(run_env("approvals_list", json))
        }
        "show" if id.is_empty() => need_id("show"),
        "show" => Plan::Run(run_env("approvals_show", json).with("M_ID", id)),
        "save" if id.is_empty() || file.is_empty() || out.is_empty() => usage(format!(
            "usage: {CLI_NAME} hostmail approvals save <id> --file <n> --out <path>"
        )),
        "save" => Plan::Run(
            run_env("approvals_save", json)
                .with("M_ID", id)
                .with("M_FILE", file)
                .with("M_OUT", out),
        ),
        "approve" if id.is_empty() => need_id("approve"),
        "approve" => Plan::Run(
            run_env("approvals_approve", json)
                .with("M_ID", id)
                .with("M_NOTE", note),
        ),
        _ if id.is_empty() => need_id("deny"),
        _ if note.is_empty() => {
            usage("deny requires --note — the reason flows back to the agent's outbox")
        }
        _ => Plan::Run(
            run_env("approvals_deny", json)
                .with("M_ID", id)
                .with("M_NOTE", note),
        ),
    }
}

/// `_mail_py`'s branch for `verb`; `None` for any other.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "approvals_list" => list_py(py_),
        "approvals_show" => show_py(py_),
        "approvals_save" => save_py(py_),
        "approvals_approve" => approve_py(py_),
        "approvals_deny" => deny_py(py_),
        _ => return None,
    })
}

fn g(d: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(d, key).at(l, MODULE)
}

fn sv(t: &str) -> Value {
    p::sv(t)
}

fn s_(v: &Value) -> String {
    py::str_of(v)
}

/// `d.get(k, "")` as `%s` prints it, at `l`.
fn s_or_empty(d: &Value, key: &str, l: u32) -> R<String> {
    Ok(s_(&p::get_or(d, key, sv("")).at(l, MODULE)?))
}

/// `human_size(n)`.
pub fn human_size(n: Option<&Value>) -> Result<String, String> {
    let v = p::or(n.cloned(), Value::Int("0".into()));
    let digits = p::py_int(&v)?;
    let small = digits.starts_with('-')
        || (digits.len() < 5 && digits.parse::<u32>().is_ok_and(|n| n < 1024));
    if small {
        return Ok(format!("{digits} bytes"));
    }
    let f: f64 = digits
        .parse()
        .ok()
        .filter(|f: &f64| f.is_finite())
        .ok_or_else(|| "OverflowError: int too large to convert to float".to_string())?;
    if f < 1024.0 * 1024.0 {
        return Ok(format!("{:.1} KiB", f / 1024.0));
    }
    Ok(format!("{:.1} MiB", f / (1024.0 * 1024.0)))
}

/// `file_line(f)`.
fn file_line(f: &Value) -> R<String> {
    const F: &str = "file_line";
    let name = p::or(attr_get(f, "name").at(line::FILE_LINE, F)?, sv("?"));
    let size = attr_get(f, "size").at(line::FILE_LINE, F)?;
    let size = human_size(size.as_ref())
        .at(line::HUMAN_SIZE, "human_size")
        .within_frame(line::FILE_LINE, F)?;
    let ct = p::or(
        attr_get(f, "contentType").at(line::FILE_LINE + 1, F)?,
        sv("?"),
    );
    let scanned = p::truthy(attr_get(f, "scanned").at(line::FILE_LINE + 2, F)?.as_ref());
    Ok(format!(
        "{} ({size}, {}{})",
        print::term_line(&name),
        print::term_line(&ct),
        if scanned { "" } else { ", not scanned" }
    ))
}

/// `shown_as(r)`.
fn shown_as(r: &Value) -> R<String> {
    const F: &str = "shown_as";
    if !p::truthy(attr_get(r, "fromName").at(line::SHOWN_AS, F)?.as_ref()) {
        return Ok(String::new());
    }
    let n = py_index(r, "fromName").at(line::SHOWN_AS, F)?;
    Ok(format!(" shown as \"{}\"", print::term_line(&n)))
}

/// A frame for the caller, on an exception from a function it called.
trait WithinFrame {
    fn within_frame(self, line: u32, name: &'static str) -> Self;
}

impl<T> WithinFrame for R<T> {
    fn within_frame(self, line: u32, name: &'static str) -> Self {
        core::Within::within(self, line, name)
    }
}

/// `", ".join(v)` at `l`.
fn joined(v: &Value, l: u32) -> R<String> {
    join(", ", v).at(l, MODULE)
}

/// The lines a pending send shares between `list` (indented by two) and
/// `show`, from `to` down to the files; `first` is the module line of
/// the `to` line.
fn detail(py_: &mut Py, r: &Value, pad: &str, first: u32, files_at: u32) -> R<()> {
    let to = p::or(g(r, "to", first)?, Value::Array(Vec::new()));
    let to = joined(&to, first)?;
    py_.print(&format!(
        "{pad}to      : {}",
        if to.is_empty() { "?" } else { to.as_str() }
    ));
    let lines: [(&str, &str, &str); 4] = [
        ("cc", "cc      : ", ""),
        ("bcc", "bcc     : ", ""),
        ("policyBcc", "bcc (owner policy): ", ""),
        (
            "replyTo",
            "reply-to: ",
            " (answers go here, not to the sender)",
        ),
    ];
    let mut l = first + 1;
    for (key, label, tail) in lines {
        if p::truthy(g(r, key, l)?.as_ref()) {
            let v = py_index(r, key).at(l + 1, MODULE)?;
            let j = joined(&v, l + 1)?;
            py_.print(&format!("{pad}{label}{j}{tail}"));
        }
        l += 2;
    }
    if p::truthy(g(r, "forwardOf", l)?.as_ref()) {
        let f = py_index(r, "forwardOf").at(l + 1, MODULE)?;
        let fl = print::forward_line(&f).within_frame(l + 1, MODULE)?;
        py_.print(&format!("{pad}forwards: {fl}"));
    }
    l += 2;
    let subject = p::or(g(r, "subject", l)?, sv("(no subject)"));
    py_.print(&format!("{pad}subject : {}", print::term_line(&subject)));
    l += 1;
    if p::truthy(g(r, "sourceDmarc", l)?.as_ref()) {
        let v = py_index(r, "sourceDmarc").at(l + 1, MODULE)?;
        let note = if p::eq_str(&v, "fail") {
            " (its sender may be forged)"
        } else {
            ""
        };
        py_.print(&format!(
            "{pad}original: DMARC {}{note}",
            print::term_line(&v)
        ));
    }
    l += 2;
    if p::truthy(g(r, "sourceCheck", l)?.as_ref()) {
        let v = py_index(r, "sourceCheck").at(l + 1, MODULE)?;
        py_.print(&format!(
            "{pad}original: {}. Its sender may be forged.",
            print::term_line(&v)
        ));
    }
    let files = p::or(g(r, "files", files_at)?, Value::Array(Vec::new()));
    for f in iter(&files).at(files_at, MODULE)? {
        let n = p::get_or(&f, "n", sv("?")).at(files_at + 1, MODULE)?;
        let fl = file_line(&f).within_frame(files_at + 1, MODULE)?;
        py_.print(&format!("{pad}file {}: {fl}", p::ljust(&s_(&n), 3)));
    }
    Ok(())
}

/// The head of a pending send: id, sender, display name, who queued it.
fn who(r: &Value, l: u32) -> R<Value> {
    Ok(match g(r, "workspace", l)?.filter(py::truthy) {
        Some(v) => v,
        None => p::or(g(r, "agentName", l)?, sv("?")),
    })
}

fn list_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/approvals/list", &[], line::LIST_CALL)?;
    let rows = p::or(g(&d, "pending", line::LIST_ROWS)?, Value::Array(Vec::new()));
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    if !py::truthy(&rows) {
        py_.print("No sends waiting for approval.");
        return Ok(());
    }
    let n = p::len(&rows).at(line::LIST_COUNT, MODULE)?;
    let count = p::get_or(&d, "count", Value::Int(n.to_string())).at(line::LIST_COUNT, MODULE)?;
    py_.print(&format!("{} send(s) waiting for approval:", s_(&count)));
    py_.print("");
    for r in iter(&rows).at(line::LIST_FOR, MODULE)? {
        let who = who(&r, line::LIST_WHO)?;
        let mut exp = String::new();
        if p::truthy(g(&r, "expiresAt", line::LIST_EXP_IF)?.as_ref()) {
            let at = py_index(&r, "expiresAt").at(line::LIST_EXP, MODULE)?;
            let u = core::until(Some(&at));
            exp = if u == "expired" {
                ", auto-denies soon".into()
            } else {
                format!(", expires {u}")
            };
        }
        let l = line::LIST_HEAD;
        let id = s_or_empty(&r, "id", l)?;
        let from = p::or(g(&r, "from", l + 1)?, sv("?"));
        let shown = shown_as(&r).within_frame(l + 1, MODULE)?;
        let ago = core::unix_ago(g(&r, "createdAt", l + 1)?.as_ref());
        py_.print(&format!(
            "{id}  from {}{shown} ({}) — queued {ago}{exp}",
            s_(&from),
            s_(&who)
        ));
        detail(py_, &r, "  ", line::LIST_TO, line::LIST_FILES)?;
        if !matches!(
            g(&r, "bodyPreview", line::LIST_PREVIEW_IF)?,
            None | Some(Value::Null)
        ) {
            let v = py_index(&r, "bodyPreview").at(line::LIST_PREVIEW_IF + 1, MODULE)?;
            py_.print(&format!("  preview : {}", print::term_line(&v)));
        }
        let id = s_or_empty(&r, "id", line::LIST_OPEN)?;
        py_.print(&format!(
            "  open    : {CLI_NAME} hostmail approvals show {id}"
        ));
        let id = s_or_empty(&r, "id", line::LIST_DECIDE + 1)?;
        py_.print(&format!(
            "  decide  : {CLI_NAME} hostmail approvals approve {id} | deny {id} --note \"...\""
        ));
        py_.print("");
    }
    Ok(())
}

fn show_py(py_: &mut Py) -> R<()> {
    let params = vec![("id".to_string(), py_.env("M_ID").to_string())];
    let d = py_.get("/cli/mail/approvals/show", &params, line::SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let m = p::or(
        g(&d, "message", line::SHOW_MESSAGE)?,
        Value::Object(Vec::new()),
    );
    let who = who(&m, line::SHOW_WHO)?;
    let l = line::SHOW_HEAD;
    let id = s_or_empty(&m, "id", l)?;
    let from = p::or(g(&m, "from", l)?, sv("?"));
    let shown = shown_as(&m).within_frame(l, MODULE)?;
    py_.print(&format!(
        "{id}  from {}{shown} ({}), waiting for approval",
        s_(&from),
        s_(&who)
    ));
    detail(py_, &m, "", line::SHOW_TO, line::SHOW_FILES)?;
    if p::truthy(g(&m, "files", line::SHOW_SAVE_IF)?.as_ref()) {
        let id = s_or_empty(&m, "id", line::SHOW_SAVE_IF + 1)?;
        py_.print(&format!(
            "          save one to look at it: {CLI_NAME} hostmail approvals save {id} --file <n> --out <path>"
        ));
    }
    py_.print("");
    let t = p::or(g(&m, "text", line::SHOW_TEXT)?, sv(""));
    py_.print(&print::term_safe(&t));
    Ok(())
}

fn save_py(py_: &mut Py) -> R<()> {
    let out = py_.env("M_OUT").to_string();
    if std::fs::symlink_metadata(&out).is_ok() {
        return Err(py_.die_with(
            "usage",
            &format!("--out '{out}' already exists — pick a new path"),
            None,
        ));
    }
    let params = vec![
        ("id".to_string(), py_.env("M_ID").to_string()),
        ("file".to_string(), py_.env("M_FILE").to_string()),
    ];
    let d = py_.call_full(
        crate::transport::urllib::Method::Get,
        "/cli/mail/approvals/show",
        &params,
        None,
        Secs::Int(120),
        None,
        line::SAVE_CALL,
    )?;
    let b = p::or(g(&d, "dataBase64", line::SAVE_DATA)?, sv(""));
    let data = b64decode(&b).at(line::SAVE_DATA, MODULE)?;
    if let Err(e) = write_new(&out, &data) {
        return Err(py_.die_with(
            "usage",
            &format!(
                "can't write --out '{out}': {}",
                pyjson::os_error(&e, &out).message
            ),
            None,
        ));
    }
    if py_.json {
        let l = line::SAVE_JSON;
        let v = core::obj(vec![
            ("ok", Value::Bool(true)),
            ("id", g(&d, "id", l)?.unwrap_or(Value::Null)),
            ("n", g(&d, "n", l)?.unwrap_or(Value::Null)),
            ("name", g(&d, "name", l)?.unwrap_or(Value::Null)),
            ("size", Value::Int(data.len().to_string())),
            ("path", sv(&out)),
        ]);
        py_.print_json(&v);
        return Ok(());
    }
    let name = p::or(g(&d, "name", line::SAVE_PRINT + 2)?, sv("?"));
    let size = human_size(Some(&Value::Int(data.len().to_string())))
        .at(line::HUMAN_SIZE, "human_size")
        .within_frame(line::SAVE_PRINT + 2, MODULE)?;
    py_.print(&format!(
        "saved {} ({size}) to {out}. It isn't opened: it would leave exactly as it is, and it may have come from outside, so treat it as untrusted.",
        print::term_safe(&name)
    ));
    Ok(())
}

/// `os.open(out, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)` and the
/// write. Off Unix the create alone: a new file only (`CREATE_NEW`, which
/// a link or anything else at the path refuses, so no link is followed),
/// and no mode to give it.
fn write_new(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(data)
}

fn approve_py(py_: &mut Py) -> R<()> {
    let mut body = vec![("id", sv(py_.env("M_ID")))];
    if !py_.env("M_NOTE").is_empty() {
        body.push(("note", sv(py_.env("M_NOTE"))));
    }
    let d = py_.post(
        "/cli/mail/approvals/approve",
        &core::obj(body),
        line::APPROVE_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::APPROVE_PRINT;
    let id = s_or_empty(&d, "id", l)?;
    let tail = match g(&d, "hint", l)?.filter(py::truthy) {
        Some(h) => h,
        None => p::get_or(&d, "status", sv("")).at(l, MODULE)?,
    };
    py_.print(&format!("{id} {}", s_(&tail)));
    Ok(())
}

fn deny_py(py_: &mut Py) -> R<()> {
    let body = core::obj(vec![
        ("id", sv(py_.env("M_ID"))),
        ("note", sv(py_.env("M_NOTE"))),
    ]);
    let d = py_.post("/cli/mail/approvals/deny", &body, line::DENY_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let id = s_or_empty(&d, "id", line::DENY_PRINT)?;
    py_.print(&format!(
        "{id} rejected — your note reaches the agent in its outbox."
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `_mail_py`'s script: line 1 is the one after the command line that
    /// feeds the heredoc.
    fn script() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/mail.sh");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let body = text
            .split_once("python3 -c \"$_K2_PY_STDIN_BOOT\"\n")
            .expect("_mail_py's command line")
            .1;
        let end = body.find("\nPYEOF\n").expect("the heredoc's end");
        body[..end].split('\n').map(str::to_string).collect()
    }

    #[test]
    fn the_lines_that_raise_are_the_shell_s() {
        let s = script();
        let pins: &[(u32, &str)] = &[
            (line::HUMAN_SIZE, "n = int(n or 0)"),
            (
                line::FILE_LINE,
                "return \"%s (%s, %s%s)\" % (term_line(f.get(\"name\")",
            ),
            (line::FILE_LINE + 1, "term_line(f.get(\"contentType\")"),
            (line::FILE_LINE + 2, "\"\" if f.get(\"scanned\") else"),
            (
                line::SHOWN_AS,
                "return \" shown as \\\"%s\\\"\" % term_line(r[\"fromName\"])",
            ),
            (
                line::LIST_CALL,
                "d = call(\"GET\", \"/cli/mail/approvals/list\")",
            ),
            (line::LIST_ROWS, "rows = d.get(\"pending\") or []"),
            (
                line::LIST_COUNT,
                "print(\"%s send(s) waiting for approval:\"",
            ),
            (line::LIST_FOR, "for r in rows:"),
            (
                line::LIST_WHO,
                "who = r.get(\"workspace\") or r.get(\"agentName\")",
            ),
            (line::LIST_EXP_IF, "if r.get(\"expiresAt\"):"),
            (line::LIST_EXP, "u = until(r[\"expiresAt\"])"),
            (
                line::LIST_HEAD,
                "print(\"%s  from %s%s (%s) — queued %s%s\"",
            ),
            (line::LIST_HEAD + 1, "r.get(\"from\") or \"?\", shown_as(r)"),
            (
                line::LIST_TO,
                "print(\"  to      : %s\" % (\", \".join(r.get(\"to\")",
            ),
            (line::LIST_CC_IF, "if r.get(\"cc\"):"),
            (line::LIST_CC_IF + 2, "if r.get(\"bcc\"):"),
            (line::LIST_CC_IF + 4, "if r.get(\"policyBcc\"):"),
            (line::LIST_CC_IF + 6, "if r.get(\"replyTo\"):"),
            (line::LIST_CC_IF + 8, "if r.get(\"forwardOf\"):"),
            (line::LIST_CC_IF + 10, "print(\"  subject : %s\""),
            (line::LIST_CC_IF + 11, "if r.get(\"sourceDmarc\"):"),
            (line::LIST_CC_IF + 13, "if r.get(\"sourceCheck\"):"),
            (line::LIST_FILES, "for f in r.get(\"files\") or []:"),
            (line::LIST_FILE, "print(\"  file %-3s: %s\""),
            (
                line::LIST_PREVIEW_IF,
                "if r.get(\"bodyPreview\") is not None:",
            ),
            (
                line::LIST_OPEN,
                "print(\"  open    : k2 hostmail approvals show",
            ),
            (
                line::LIST_DECIDE + 1,
                "% (r.get(\"id\", \"\"), r.get(\"id\", \"\")))",
            ),
            (
                line::SHOW_CALL,
                "d = call(\"GET\", \"/cli/mail/approvals/show\"",
            ),
            (line::SHOW_MESSAGE, "m = d.get(\"message\") or {}"),
            (
                line::SHOW_WHO,
                "who = m.get(\"workspace\") or m.get(\"agentName\")",
            ),
            (
                line::SHOW_HEAD,
                "print(\"%s  from %s%s (%s), waiting for approval\"",
            ),
            (
                line::SHOW_TO,
                "print(\"to      : %s\" % (\", \".join(m.get(\"to\")",
            ),
            (line::SHOW_CC_IF, "if m.get(\"cc\"):"),
            (line::SHOW_CC_IF + 8, "if m.get(\"forwardOf\"):"),
            (line::SHOW_CC_IF + 10, "print(\"subject : %s\""),
            (line::SHOW_CC_IF + 11, "if m.get(\"sourceDmarc\"):"),
            (line::SHOW_CC_IF + 13, "if m.get(\"sourceCheck\"):"),
            (line::SHOW_FILES, "for f in m.get(\"files\") or []:"),
            (line::SHOW_FILE, "print(\"file %-3s: %s\""),
            (line::SHOW_SAVE_IF, "if m.get(\"files\"):"),
            (line::SHOW_TEXT, "print(term_safe(m.get(\"text\") or \"\"))"),
            (
                line::SAVE_CALL,
                "d = call(\"GET\", \"/cli/mail/approvals/show\",",
            ),
            (
                line::SAVE_DATA,
                "data = base64.b64decode(d.get(\"dataBase64\")",
            ),
            (
                line::SAVE_JSON,
                "print(dumps({\"ok\": True, \"id\": d.get(\"id\")",
            ),
            (
                line::SAVE_PRINT + 2,
                "% (term_safe(d.get(\"name\") or \"?\")",
            ),
            (
                line::APPROVE_CALL,
                "d = call(\"POST\", \"/cli/mail/approvals/approve\"",
            ),
            (
                line::APPROVE_PRINT,
                "print(\"%s %s\" % (d.get(\"id\", \"\"), d.get(\"hint\")",
            ),
            (
                line::DENY_CALL,
                "d = call(\"POST\", \"/cli/mail/approvals/deny\",",
            ),
            (line::DENY_PRINT, "print(\"%s rejected — your note reaches"),
        ];
        for (n, text) in pins {
            let got = s[*n as usize - 1].trim();
            assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
        }
    }

    fn hint(words: &[&str]) -> String {
        match parse("approvals", words) {
            Plan::Usage(h) => h,
            other => panic!("{words:?}: {other:?}"),
        }
    }

    #[test]
    fn the_words_are_cmd_mail_approvals_s() {
        let verb = |words: &[&str]| match parse("approvals", words) {
            Plan::Run(e) => e.get("M_VERB").to_string(),
            other => panic!("{words:?}: {other:?}"),
        };
        assert_eq!(verb(&[]), "approvals_list");
        assert_eq!(verb(&["--json"]), "approvals_list");
        assert_eq!(verb(&[""]), "approvals_list");
        assert_eq!(verb(&["show", "out_1"]), "approvals_show");
        assert_eq!(
            verb(&["save", "out_1", "--file", "1", "--out", "f"]),
            "approvals_save"
        );
        assert_eq!(verb(&["approve", "out_1"]), "approvals_approve");
        assert_eq!(verb(&["deny", "out_1", "--note", "no"]), "approvals_deny");
        assert_eq!(
            parse("approvals", &["list", "-h"]),
            Plan::Help("hostmail/approvals")
        );
        assert_eq!(
            hint(&["deny", "out_1"]),
            "deny requires --note — the reason flows back to the agent's outbox"
        );
        assert_eq!(
            hint(&["show", "a", "b"]),
            "unexpected extra argument 'b' — one <id> only"
        );
        assert_eq!(hint(&["show", "--note"]), "--note requires a value");
        assert_eq!(
            hint(&["save", "a", "--out"]),
            "--out requires a path to write the file to"
        );
    }

    /// Under any brand: the usage lines name the brand's command, never
    /// another.
    #[test]
    fn usage_lines_name_the_brand_s_command() {
        for words in [
            &["nope"][..],
            &["show"],
            &["approve"],
            &["deny"],
            &["save", "x"],
            &["list", "x"],
            &["list", "--bogus"],
            &["show", "--file"],
        ] {
            let h = hint(words);
            assert!(
                h.contains(&format!("{CLI_NAME} hostmail approvals")),
                "{words:?}: {h}"
            );
            if CLI_NAME != "k2" {
                assert!(!h.contains("k2 ") && !h.contains("k5 "), "{words:?}: {h}");
            }
        }
    }

    #[test]
    fn human_size_is_python_s() {
        let size = |n: &str| human_size(Some(&Value::Int(n.into()))).unwrap();
        assert_eq!(size("0"), "0 bytes");
        assert_eq!(size("1023"), "1023 bytes");
        assert_eq!(size("-5"), "-5 bytes");
        assert_eq!(size("1024"), "1.0 KiB");
        assert_eq!(size("1536"), "1.5 KiB");
        assert_eq!(size("1048576"), "1.0 MiB");
        assert_eq!(human_size(None).unwrap(), "0 bytes");
        assert_eq!(human_size(Some(&Value::Null)).unwrap(), "0 bytes");
    }
}
