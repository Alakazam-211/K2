//! `compute provenance|verify ...`, as `cmd_compute_provenance` answers
//! them (`cli/src/compute-provenance.sh`; `receipt` is
//! [`super::super::reads`]'):
//!
//! - `provenance <job> [--out FILE] [--json]`: the job's build statement
//!   (the `.intoto.jsonl` line as it is, or written to FILE with what it
//!   names);
//! - `verify <job | FILE.intoto.jsonl> [--file PATH] [--owner-key FP]
//!   [--json]`: the daemon's check of a statement, exit 1 when it is not
//!   verified.

use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{dumps, object, str_value, Cp, Words};
use crate::exit::Exit;
use crate::families::{py_space, raised, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::out::pylib::json as pyjson;
use crate::transport::encode::quote;
use k2_client::{brand::CLI_NAME, cli_name};

/// `cli/src/compute-provenance.sh`.
pub const SRC: &str = include_str!("../../../../../../cli/src/compute-provenance.sh");

/// The `line!` lookup for a Python with the shell's `'"'"'` in it.
macro_rules! line_q {
    ($after:expr, $marker:literal) => {
        const { super::py_line_q(SRC, $after, $marker) }
    };
}

pub const OUT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute provenance <job> --out FILE"
);
pub const FILE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute verify <job | FILE.intoto.jsonl> --file PATH"
);
pub const KEY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute verify <job | FILE.intoto.jsonl> --owner-key FP"
);
pub const PROVENANCE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute provenance <job> [--out FILE] [--json]"
);
pub const VERIFY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute verify <job | FILE.intoto.jsonl> [--file PATH] [--owner-key FP] [--json]"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Provenance {
        job: String,
        out: String,
        json: bool,
    },
    Verify {
        target: String,
        file: String,
        owner_key: String,
        json: bool,
    },
}

/// The words after `provenance` or `verify`.
pub fn parse(verb: &str, rest: &[&str]) -> Words<Ask> {
    let (mut json, mut out, mut file, mut owner_key, mut target) =
        (false, "", "", "", String::new());
    let mut i = 0;
    while let Some(w) = rest.get(i) {
        match *w {
            "--json" => {
                json = true;
                i += 1;
            }
            "--out" | "--file" | "--owner-key" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                let usage = match *w {
                    "--out" => {
                        out = v;
                        OUT_USAGE
                    }
                    "--file" => {
                        file = v;
                        FILE_USAGE
                    }
                    _ => {
                        owner_key = v;
                        KEY_USAGE
                    }
                };
                if v.is_empty() {
                    return Words::Usage(usage.into());
                }
                i += 2;
            }
            w if w.starts_with('-') => {
                return Words::Usage(format!(
                    "{CLI_NAME} compute {verb}: unknown flag {w} ({CLI_NAME} compute --help)"
                ))
            }
            w => {
                if !target.is_empty() {
                    return Words::Usage(format!("{CLI_NAME} compute {verb}: one job at a time"));
                }
                target = w.to_string();
                i += 1;
            }
        }
    }
    if verb == "provenance" {
        if target.is_empty() || !file.is_empty() || !owner_key.is_empty() {
            return Words::Usage(PROVENANCE_USAGE.into());
        }
        return Words::Ask(Ask::Provenance {
            job: target,
            out: out.to_string(),
            json,
        });
    }
    if target.is_empty() || !out.is_empty() {
        return Words::Usage(VERIFY_USAGE.into());
    }
    Words::Ask(Ask::Verify {
        target,
        file: file.to_string(),
        owner_key: owner_key.to_string(),
        json,
    })
}

/// The requests and what they print.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    match a {
        Ask::Provenance { job, out, json } => provenance(cp, job, out, *json),
        Ask::Verify {
            target,
            file,
            owner_key,
            json,
        } => verify(cp, target, file, owner_key, *json),
    }
}

fn shown(mut cx: Cx, r: Result<(), Raise>) -> Shown {
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e),
    }
}

fn provenance(cp: &mut Cp<'_>, job: &str, out_path: &str, json: bool) -> Exit {
    let query = format!("job={}", quote(job));
    let answer = match cp.call(false, "/cli/compute/jobs/provenance", &query, false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if json {
        return super::super::writes::print_body(&answer);
    }
    let mut cx = Cx::new(0);
    if out_path.is_empty() {
        // `sys.stdout.write(json.load(sys.stdin)["line"])`, on line 1.
        let r = render::load(&cx, &answer, 1)
            .and_then(|v| cx.at(1, p::idx(&v, "line")))
            .and_then(|l| match &l {
                Value::Str(_) => Ok(p::str_of(&l)),
                other => Err(cx.raise(
                    1,
                    format!(
                        "TypeError: write() argument must be str, not {}",
                        p::tname(other)
                    ),
                )),
            });
        let r = r.map(|t| cx.out.push_str(&t));
        return shown(cx, r).print();
    }
    let r = written(&mut cx, &answer, out_path);
    shown(cx, r).print()
}

/// The `--out` Python: the line written to `path`, then what it names.
fn written(cx: &mut Cx, body: &[u8], path: &str) -> Result<(), Raise> {
    const AFTER: &str = "sys.stdout.write(json.load(sys.stdin)[\"line\"])'";
    let l = line_q!(AFTER, "v = json.load(sys.stdin)");
    let v = render::load(cx, body, l)?;
    let l = line_q!(AFTER, "with open(path, \"w\") as f:");
    let mut f = std::fs::File::create(path).map_err(|e| {
        let f = pyjson::os_error(&e, path);
        cx.raise(l, f.last_line())
    })?;
    let l = line_q!(AFTER, "f.write(v[\"line\"])");
    let text = cx.at(l, p::idx(&v, "line"))?;
    let Value::Str(_) = &text else {
        return Err(cx.raise(
            l,
            format!(
                "TypeError: write() argument must be str, not {}",
                p::tname(&text)
            ),
        ));
    };
    {
        use std::io::Write;
        f.write_all(p::str_of(&text).as_bytes()).map_err(|e| {
            let f = pyjson::os_error(&e, path);
            cx.raise(l, f.last_line())
        })?;
    }
    drop(f);
    let l = line_q!(AFTER, "subs = v.get(\"subjects\") or []");
    let subs = p::or(cx.at(l, p::get(&v, "subjects"))?, Value::Array(vec![]));
    let l = line_q!(AFTER, "print(\"wrote %s: job %s");
    let short = cx.at(l, p::idx(&v, "short"))?;
    let node = p::or(cx.at(l, p::get(&v, "node"))?, s("?"));
    let n = cx.at(l, p::len(&subs))?;
    let fmt = "wrote %s: job %s's build statement, signed by node %s, naming %d file(s):";
    let t = cx.at(l, p::pct(fmt, &[s(path), short, node, p::int(n as i128)]))?;
    cx.print(&t);
    let l = line_q!(AFTER, "for s in subs:");
    for x in cx.at(l, p::iter(&subs))? {
        let l = line_q!(AFTER, "print(\"  %s  sha256 %s\"");
        let name = cx.at(l, p::idx(&x, "name"))?;
        let sum = cx.at(l, p::idx(&x, "sha256"))?;
        let t = cx.at(l, p::pct("  %s  sha256 %s", &[name, sum]))?;
        cx.print(&t);
    }
    cx.print(&format!(
        "check it: {CLI_NAME} compute verify {path} [--file <one of those files>]"
    ));
    Ok(())
}

/// Python's `str.strip()` left something.
fn not_blank(l: &str) -> bool {
    !l.trim_matches(py_space).is_empty()
}

/// The `body` Python: the request's JSON, or what it stops with (its
/// `sys.exit` line or traceback on stderr; the shell then exits 2).
fn verify_body(target: &str, file: &str, owner: &str) -> Result<String, String> {
    let mut o: Vec<(&str, Value)> = vec![];
    if std::fs::metadata(target).is_ok_and(|m| m.is_file()) {
        let text = pyjson::read_text_file(target).map_err(|f| traceback(&f))?;
        let lines: Vec<String> = p::py_splitlines(&text)
            .into_iter()
            .filter(|l| not_blank(l))
            .collect();
        if lines.len() != 1 {
            return Err(format!(
                "{CLI_NAME} compute verify: {target} holds {} lines; a .intoto.jsonl file from {CLI_NAME} compute provenance holds one statement\n",
                lines.len()
            ));
        }
        match pyjson::loads(&lines[0]) {
            Ok(v) => o.push(("envelope", v)),
            Err(f) if matches!(f.class, "JSONDecodeError" | "ValueError") => {
                return Err(format!(
                    "{CLI_NAME} compute verify: {target} is not a statement line (JSON): {}\n",
                    f.message
                ))
            }
            Err(f) => return Err(traceback(&f)),
        }
    } else {
        o.push(("job", str_value(target)));
    }
    if !file.is_empty() {
        let data = std::fs::read(file).map_err(|e| traceback(&pyjson::os_error(&e, file)))?;
        o.push(("sha256", s(&k2_client::release_sums::sha256_hex(&data))));
        let base = file.rsplit('/').next().unwrap_or(file);
        o.push(("file", s(base)));
    }
    if !owner.is_empty() {
        o.push(("ownerKey", s(owner)));
    }
    Ok(dumps(&object(o)))
}

/// A traceback of Python's `-c` program ending in `f`.
fn traceback(f: &pyjson::Failed) -> String {
    Raise::at(1, f.last_line()).text()
}

fn verify(cp: &mut Cp<'_>, target: &str, file: &str, owner: &str, json: bool) -> Exit {
    if !file.is_empty() && !std::fs::metadata(file).is_ok_and(|m| m.is_file()) {
        out::stderr_line(&format!(
            "{CLI_NAME} compute verify: --file {file}: no such file"
        ));
        return Exit::Usage;
    }
    let payload = match verify_body(target, file, owner) {
        Ok(b) => b,
        Err(text) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return Exit::Usage;
        }
    };
    let answer = match cp.call(true, "/cli/compute/verify", &payload, false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let exit = if json {
        super::super::writes::print_body(&answer)
    } else {
        let mut cx = Cx::new(0);
        let r = checks(&mut cx, &answer);
        shown(cx, r).print()
    };
    if exit != Exit::Ok {
        return exit;
    }
    // `sys.exit(0 if json.load(sys.stdin)["verified"] else 1)`.
    let cx = Cx::new(0);
    let r = render::load(&cx, &answer, 1).and_then(|v| cx.at(1, p::idx(&v, "verified")));
    match r {
        Ok(v) if p::truthy(&v) => Exit::Ok,
        Ok(_) => Exit::Error,
        Err(e) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
            Exit::Error
        }
    }
}

/// The verify answer's reader.
fn checks(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "_compute_call POST /cli/compute/verify";
    let v = render::load(cx, body, line_q!(AFTER, "v = json.load(sys.stdin)"))?;
    let l = line_q!(AFTER, "what = \"job %s");
    let short = cx.at(l, p::idx(&v, "short"))?;
    let node = p::or(cx.at(l, p::get(&v, "node"))?, s("?"));
    let what = cx.at(l, p::pct("job %s's build statement (node %s)", &[short, node]))?;
    let l = line_q!(AFTER, "print((\"verified: \" + what)");
    let t = if p::truthy(&cx.at(l, p::idx(&v, "verified"))?) {
        format!("verified: {what}")
    } else {
        let failed = cx.at(l, p::get(&v, "failed"))?;
        cx.at(l, p::pct("NOT verified: %s: %s", &[s(&what), failed]))?
    };
    cx.print(&t);
    let l = line_q!(AFTER, "for c in v[\"checks\"]:");
    for c in cx.at(l, p::idx(&v, "checks").and_then(|c| p::iter(&c)))? {
        let l = line_q!(AFTER, "print(\"  %-8s %-10s %s\"");
        let ok = if p::truthy(&cx.at(l, p::idx(&c, "ok"))?) {
            "checked"
        } else {
            "FAILED"
        };
        let check = cx.at(l, p::idx(&c, "check"))?;
        let detail = cx.at(l, p::idx(&c, "detail"))?;
        let t = cx.at(l, p::pct("  %-8s %-10s %s", &[s(ok), check, detail]))?;
        cx.print(&t);
    }
    let l = line_q!(AFTER, "print(\"not checked: \"");
    let n = cx.at(l, p::idx(&v, "notChecked"))?;
    let t = cx.at(l, p::add(&s("not checked: "), &n))?;
    cx.print(&p::str_of(&t));
    Ok(())
}
