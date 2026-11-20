//! `ci signer …` (`signers`), as `cmd_ci_signer` answers it
//! (`cli/src/ci.sh`): pair a signing machine with this server. `list`
//! reads; `add`, `reveal`, `finish` and `remove` are a person's, at this
//! server: in a session they are refused (exit 3) after the usage checks
//! and before any prompt or request. `add` and `reveal` ask the vault for
//! a confirm token first. `add --code-stdin` reads the pairing code from
//! stdin (one line; no echo on a terminal); a `<CODE.FP>` word still works
//! and prints one note. The code and the token go only in the body.

use super::super::compute::admin::node::flows::confirm;
use super::super::compute::writes::{self, dumps, in_session, object, str_value, Cp};
use super::Parsed;
use crate::confirm::confirm_token;
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out::json::{self, Value};
use crate::out::pylib::json::{loads, os_error, read_text_file};
use crate::out::{self, py};
use crate::prompt;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::product_name;
use std::path::Path;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    List,
    Add {
        code: String,
        code_stdin: bool,
        hello: String,
        out: String,
    },
    Reveal {
        id: String,
        rev: String,
    },
    Finish {
        id: String,
        done: String,
    },
    Remove(String),
}

/// `ci signer add`'s usage.
pub fn add_usage() -> String {
    format!(
        "usage: {CLI} ci signer add --code-stdin --hello <pair-begin output file> [--out offer.json] < code-file (or <CODE.FP> instead of --code-stdin, visible to every account on this computer)"
    )
}

const PERSON: &str = concat!(
    "{\"error\":\"person_only\",\"hint\":\"signer pairing is a person's, at this server: a terminal outside ",
    product_name!(),
    " (Touch ID / vault password)\"}"
);

/// `cmd_ci_signer`'s words: `sub` is `signer` or `signers`.
pub fn parse(sub: &str, rest: &[&str]) -> Parsed {
    let (verb, rest) = if sub == "signers" {
        ("list", rest)
    } else {
        match rest.split_first() {
            Some((v, r)) => (*v, r),
            None => ("list", rest),
        }
    };
    let usage = |l: String| Parsed::Usage(l);
    match verb {
        "list" | "ls" => Parsed::Signer(Ask::List),
        "add" => {
            let mut code = String::new();
            let mut rest = rest;
            if let Some(first) = rest.first() {
                if !first.is_empty() && !first.starts_with('-') {
                    code = first.to_string();
                    rest = &rest[1..];
                }
            }
            let (mut code_stdin, mut hello, mut out) = (false, String::new(), "k2-signer-offer.json".to_string());
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--code-stdin" => code_stdin = true,
                    f @ ("--hello" | "--out") => {
                        // The shell's `shift 2` with no value never ends
                        // (a bash bug, reported); this stops on the usage.
                        let Some(v) = rest.get(i + 1) else {
                            return usage(add_usage());
                        };
                        if f == "--hello" {
                            hello = v.to_string();
                        } else {
                            out = v.to_string();
                        }
                        i += 1;
                    }
                    _ => return usage(add_usage()),
                }
                i += 1;
            }
            if code_stdin && !code.is_empty() {
                return usage(format!(
                    "{CLI} ci signer add: --code-stdin or <CODE.FP>, not both"
                ));
            }
            Parsed::Signer(Ask::Add {
                code,
                code_stdin,
                hello,
                out,
            })
        }
        "reveal" | "finish" => {
            let id = rest.first().copied().unwrap_or("").to_string();
            let rest = if id.is_empty() { rest } else { &rest[1..] };
            let flag = if verb == "reveal" { "--reveal" } else { "--done" };
            let file = if rest.first() == Some(&flag) {
                rest.get(1).copied().unwrap_or("").to_string()
            } else {
                String::new()
            };
            Parsed::Signer(if verb == "reveal" {
                Ask::Reveal { id, rev: file }
            } else {
                Ask::Finish { id, done: file }
            })
        }
        "remove" | "rm" => match rest.first() {
            Some(fp) if !fp.is_empty() => Parsed::Signer(Ask::Remove(fp.to_string())),
            _ => usage(format!(
                "usage: {CLI} ci signer remove <signer fingerprint>"
            )),
        },
        _ => usage(format!(
            "usage: {CLI} ci signer [list|add|reveal|finish|remove] ({CLI} ci --help)"
        )),
    }
}

fn is_file(p: &str) -> bool {
    !p.is_empty() && Path::new(p).is_file()
}

fn usage(line: String) -> Exit {
    out::stderr_line(&line);
    Exit::Usage
}

/// `python3 -c '… json.load(open(path)) …; print(json.dumps(…))'`: the
/// value, or the traceback (then the shell's usage line, exit 2).
fn read_json(path: &str, hello: bool) -> Result<Value, String> {
    let text = read_text_file(path).map_err(|f| Raise::at(1, f.last_line()).text())?;
    let v = loads(&text).map_err(|f| Raise::at(1, f.last_line()).text())?;
    if !hello {
        return Ok(v);
    }
    match crate::families::py_get(&v, "hello", 1) {
        Ok(Some(h)) => Ok(h.clone()),
        Ok(None) => Ok(v.clone()),
        Err(r) => Err(r.text()),
    }
}

fn person(cp: &Cp<'_>) -> Result<(), Exit> {
    if in_session(&cp.client.auth) {
        out::stderr_line(PERSON);
        return Err(Exit::Refused);
    }
    Ok(())
}

/// The one-line Python after a signer call; its exit is the command's
/// (`list`'s is lost: `return 0`).
fn reader(body: &[u8], f: impl FnOnce(&Value, &mut String) -> Result<(), Raise>) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let v = match loads(&raw) {
        Ok(v) => v,
        Err(e) => return raised(String::new(), String::new(), &Raise::at(1, e.last_line())),
    };
    let mut o = String::new();
    match f(&v, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn ix(v: &Value, k: &str, l: u32) -> Result<Value, Raise> {
    crate::out::pylib::ops::index(v, k, l).cloned()
}

fn s(v: &Value) -> String {
    py::str_of(v)
}

/// The signer verbs.
pub fn send(cp: &mut Cp<'_>, ask: &Ask, sock: &Path) -> Exit {
    match ask {
        Ask::List => {
            let b = match cp.call(false, "/cli/ci/signers", "", false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            // `set -e` with pipefail: a reader that raises ends it with 1.
            reader(&b, |v, o| {
                let fp = crate::families::py_get(v, "controllerFp", 3)?
                    .cloned()
                    .unwrap_or(Value::Null);
                o.push_str(&format!("this server (controller): {}\n", s(&fp)));
                let signers = ix(v, "signers", 4)?;
                for x in crate::families::py_iter(&signers, 4)? {
                    o.push_str(&format!(
                        "signer {}  {}  paired by {}\n",
                        s(&ix(&x, "signerFp", 5)?),
                        s(&ix(&x, "label", 5)?),
                        s(&ix(&x, "pairedBy", 5)?)
                    ));
                }
                if !py::truthy(&ix(v, "signers", 6)?) {
                    o.push_str(
                        "no signer paired (on the signing machine: sudo k2-signer-admin pair-begin)\n",
                    );
                }
                Ok(())
            })
            .print()
        }
        Ask::Add {
            code,
            code_stdin,
            hello,
            out: out_file,
        } => {
            if !((!code.is_empty() || *code_stdin) && is_file(hello)) {
                return usage(add_usage());
            }
            if let Err(e) = person(cp) {
                return e;
            }
            let code = if *code_stdin {
                let secret = prompt::read_secret_stdin("Pairing code: ");
                let mut bytes = secret.expose().to_vec();
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                if bytes.is_empty() {
                    return usage(format!(
                        "{CLI} ci signer add: --code-stdin: no code on stdin"
                    ));
                }
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                out::stderr_line(
                    "note: <CODE.FP> is visible to every account on this computer while the command runs; use --code-stdin",
                );
                code.clone()
            };
            let prefix = code.rsplit('.').next().unwrap_or("").to_string();
            let hj = match read_json(hello, true) {
                Ok(v) => v,
                Err(tb) => {
                    let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
                    return usage("usage: --hello is the JSON pair-begin printed".into());
                }
            };
            let Some(tok) = confirm_token(
                &mut confirm(cp, sock),
                "ci-signer",
                &format!("add:{prefix}"),
                "Pair a release signer with this server",
            ) else {
                return Exit::Refused;
            };
            let body = dumps(&object(vec![
                ("code", str_value(&code)),
                ("confirmToken", str_value(&tok)),
                ("hello", hj),
            ]));
            drop(tok);
            let b = match cp.call(true, "/cli/ci/signer/add", &body, true) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let path = out_file.clone();
            reader(&b, |v, o| {
                let offer = ix(v, "offer", 1)?;
                let text = json::to_string(&offer, &json::Options::default()).unwrap_or_default();
                std::fs::write(&path, text)
                    .map_err(|e| Raise::at(1, os_error(&e, &path).last_line()))?;
                let pairing = s(&ix(v, "pairing", 1)?);
                o.push_str(&format!(
                    "pairing {pairing}: offer written to {path}\non the signing machine: sudo k2-signer-admin pair-reveal < {path} > reveal.json\nthen here: {CLI} ci signer reveal {pairing} --reveal reveal.json\n"
                ));
                Ok(())
            })
            .print()
        }
        Ask::Reveal { id, rev } => {
            if id.is_empty() || !is_file(rev) {
                return usage(format!(
                    "usage: {CLI} ci signer reveal <pairing> --reveal reveal.json"
                ));
            }
            if let Err(e) = person(cp) {
                return e;
            }
            let rj = match read_json(rev, false) {
                Ok(v) => v,
                Err(tb) => {
                    let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
                    return usage("usage: --reveal is the JSON pair-reveal printed".into());
                }
            };
            let Some(tok) = confirm_token(
                &mut confirm(cp, sock),
                "ci-signer",
                id,
                "Show the signer pairing code",
            ) else {
                return Exit::Refused;
            };
            let body = dumps(&object(vec![
                ("confirmToken", str_value(&tok)),
                ("pairing", str_value(id)),
                ("reveal", rj),
            ]));
            drop(tok);
            let b = match cp.call(true, "/cli/ci/signer/reveal", &body, true) {
                Ok(b) => b,
                Err(e) => return e,
            };
            reader(&b, |v, o| {
                let sas = s(&ix(v, "sas", 1)?);
                let pairing = s(&ix(v, "pairing", 1)?);
                o.push_str(&format!(
                    "\n    {sas}\n\nType these 6 digits at the signing machine: sudo k2-signer-admin pair-accept <digits> > done.json\nthen here: {CLI} ci signer finish {pairing} --done done.json\n"
                ));
                Ok(())
            })
            .print()
        }
        Ask::Finish { id, done } => {
            if id.is_empty() || !is_file(done) {
                return usage(format!(
                    "usage: {CLI} ci signer finish <pairing> --done done.json"
                ));
            }
            if let Err(e) = person(cp) {
                return e;
            }
            let dj = match read_json(done, false) {
                Ok(v) => v,
                Err(tb) => {
                    let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
                    return usage("usage: --done is the JSON pair-accept printed".into());
                }
            };
            let body = dumps(&object(vec![("pairing", str_value(id)), ("done", dj)]));
            let b = match cp.call(true, "/cli/ci/signer/finish", &body, true) {
                Ok(b) => b,
                Err(e) => return e,
            };
            reader(&b, |v, o| {
                o.push_str(&format!(
                    "paired: signer {} ({})\n",
                    s(&ix(v, "signer", 1)?),
                    s(&ix(v, "label", 1)?)
                ));
                Ok(())
            })
            .print()
        }
        Ask::Remove(fp) => {
            if let Err(e) = person(cp) {
                return e;
            }
            let body = writes::body_of(&[("signer", fp.as_str())]);
            match cp.call(true, "/cli/ci/signer/remove", &body, true) {
                Ok(_) => Shown::out("removed\n").print(),
                Err(e) => e,
            }
        }
    }
}
