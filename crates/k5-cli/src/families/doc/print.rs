//! The verbs whose answer `_doc_print_json_or`'s readers print: their
//! words, their one call and the reader's lines, as `cmd_doc`'s arms
//! (`cli/src/doc.sh`) have them.

use super::py::{self, at, d, gt, gt_or, ix, left, local_ms, num, rows, sg, so, st, tr, Pr, R};
use super::{w, Ctx, Step, B};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::Value;
use crate::out::py as opy;
use crate::out::pylib::ops;
use crate::out::pylib::path as pypath;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// `x="${2:-}"; shift 2`: the value, or `set -e` ending the command
/// (exit 1, nothing printed) when there is none.
pub fn v2(words: &[String], i: usize) -> Result<String, Exit> {
    words.get(i + 1).cloned().ok_or(Exit::Error)
}

/// `--flag=value`'s value.
pub fn eq<'a>(a: &'a str, flag: &str) -> Option<&'a str> {
    a.strip_prefix(flag).and_then(|r| r.strip_prefix('='))
}

fn digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// `v["doc"]["path"]`.
fn doc_path(v: &Value, line: u32) -> R<String> {
    Ok(st(ix(ix(v, "doc", line)?, "path", line)?))
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `x.get(k) or {}`.
fn dict_or(v: &Value, k: &str, line: u32) -> R<Value> {
    Ok(gt_or(v, k, line)?.cloned().unwrap_or_else(empty_dict))
}

/// `"%s" % v.get(k, default)`.
fn sg_def(v: &Value, k: &str, def: &str, line: u32) -> R<String> {
    Ok(gt(v, k, line)?.map_or_else(|| def.to_string(), st))
}

fn is_zero(x: Option<&Value>) -> bool {
    x.is_some_and(|x| ops::py_eq(x, &Value::Int("0".into())))
}

fn is_str(x: Option<&Value>, s: &str) -> bool {
    matches!(x, Some(Value::Str(t)) if t.as_str() == Some(s))
}

/// The `", ".join(grants)` of a share.
fn grants(sh: &Value, line: u32) -> R<String> {
    let mut g = vec!["read".to_string()];
    for k in ["write", "history", "comment"] {
        if tr(sh, k, line)? {
            g.push(k.to_string());
        }
    }
    Ok(g.join(", "))
}

pub fn view(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut reg, mut has) = (String::new(), false);
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = &words[i];
        if a == "--region" {
            has = true;
            reg = v2(words, i)?;
            i += 2;
            continue;
        }
        if let Some(v) = eq(a, "--region") {
            has = true;
            reg = v.to_string();
        } else {
            rest.push(a.clone());
        }
        i += 1;
    }
    if !has {
        return super::tool::tool(ctx, "view", &rest);
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("view", &args)?;
    let body = ctx.body_arg(&path, &[("region", w(reg))]);
    let b = ctx.call(true, "/cli/docs/view", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let r = gt_or(v, "region", 1)?.map_or_else(|| "nothing in particular".to_string(), st);
        pr.line(&format!("viewing {r} in {}", doc_path(v, 1)?));
        Ok(Exit::Ok)
    }))
}

/// A duration's number and unit by `re.fullmatch(r"(\d+)(<units>)", s.strip())`.
fn duration<'a>(s: &'a str, units: &[&'a str], optional: bool) -> Option<(u64, &'a str)> {
    let t = s.trim_matches(crate::families::py_space);
    let n = t.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    let (num, unit) = t.split_at(n);
    let unit = units.iter().find(|u| **u == unit)?;
    if unit.is_empty() && !optional {
        return None;
    }
    num.parse::<u64>().ok().map(|x| (x, *unit))
}

/// `--ttl` of `claim` (milliseconds), `None` for a bad one.
pub fn ttl_ms(s: &str) -> Option<u64> {
    let (n, u) = duration(s, &["", "ms", "s", "m"], true)?;
    if n == 0 {
        return None;
    }
    n.checked_mul(match u {
        "s" => 1000,
        "m" => 60000,
        _ => 1,
    })
}

pub fn lease(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let mut action = if verb == "unlock" {
        "release"
    } else {
        "acquire"
    };
    let mut ttl = String::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--ttl" => {
                ttl = v2(words, i)?;
                i += 2;
                continue;
            }
            "--renew" => action = "renew",
            "--release" => action = "release",
            "--break" => action = "break",
            _ => match eq(a, "--ttl") {
                Some(v) => ttl = v.to_string(),
                None => rest.push(a.to_string()),
            },
        }
        i += 1;
    }
    let mut secs = None;
    if !ttl.is_empty() {
        let got = duration(&ttl, &["", "s", "m", "h"], true)
            .filter(|(n, _)| *n != 0)
            .and_then(|(n, u)| {
                n.checked_mul(match u {
                    "m" => 60,
                    "h" => 3600,
                    _ => 1,
                })
            });
        match got {
            Some(s) => secs = Some(s),
            None => return Err(ctx.usage("--ttl takes a duration: 90, 90s, 5m, 1h (at most 1h)")),
        }
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path(verb, &args)?;
    let mut kv = vec![("action", w(action))];
    if let Some(s) = secs {
        kv.push(("ttlSecs", w(format!("@int:{s}"))));
    }
    let body = ctx.body_arg(&path, &kv);
    let b = ctx.call(true, "/cli/docs/lease", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let a = gt(v, "action", 4)?.cloned();
        let p = doc_path(v, 4)?;
        let until = |l: &Value| -> R<String> {
            let exp = ix(l, "expiresMs", 6)?;
            let left = ((num(exp, "-", "float", 6)? - py::now_ms()) / 1000.0)
                .trunc()
                .max(0.0) as i64;
            Ok(format!(
                "until {} ({}m {}s)",
                local_ms(exp, "%H:%M:%S", 7)?,
                left.div_euclid(60),
                left.rem_euclid(60)
            ))
        };
        let a = a.as_ref();
        if is_str(a, "acquire") || is_str(a, "renew") {
            let what = if is_str(a, "acquire") {
                "leased"
            } else {
                "renewed the lease on"
            };
            let u = until(ix(v, "lease", 9)?)?;
            pr.line(&format!(
                "{what} {p} {u}; others can read, their writes wait (exit 5)"
            ));
        } else if is_str(a, "release") {
            pr.line(&if tr(v, "released", 11)? {
                format!("released the lease on {p}")
            } else {
                format!("you held no lease on {p}")
            });
        } else {
            let b = gt(v, "broken", 13)?;
            pr.line(&match b.filter(|b| opy::truthy(b)) {
                Some(b) => format!("broke {}'s lease on {p}", sg(b, "label", 14)?),
                None => format!("nobody held a lease on {p}"),
            });
        }
        py::exit_int(gt(v, "exit", 15)?.unwrap_or(&Value::Int("0".into())), 15)
    }))
}

pub fn claim(ctx: &mut Ctx, words: &[String]) -> Step {
    let node = words
        .iter()
        .any(|a| a == "--node" || a.starts_with("--node="))
        || words.last().is_some_and(|l| {
            let b = l.as_bytes();
            b.len() >= 4
                && b[b.len() - 4] == b'.'
                && b[b.len() - 3].eq_ignore_ascii_case(&b'p')
                && b[b.len() - 2].eq_ignore_ascii_case(&b's')
                && matches!(b[b.len() - 1], b'd' | b'D' | b'b' | b'B')
        });
    if node {
        return super::tool::node_claim(ctx, words);
    }
    let mut action = "take";
    let (mut lines, mut ttl, mut note, mut hold, mut id) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        let slot = match a {
            "--lines" => Some(&mut lines),
            "--ttl" => Some(&mut ttl),
            "--note" => Some(&mut note),
            "--id" => Some(&mut id),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = v2(words, i)?;
            i += 2;
            continue;
        }
        match a {
            "--hold" => hold = "@true".into(),
            "--no-hold" => hold = "@false".into(),
            "--renew" => action = "renew",
            "--release" => action = "release",
            _ => {
                if let Some(v) = eq(a, "--lines") {
                    lines = v.into();
                } else if let Some(v) = eq(a, "--ttl") {
                    ttl = v.into();
                } else if let Some(v) = eq(a, "--note") {
                    note = v.into();
                } else if let Some(v) = eq(a, "--id") {
                    id = v.into();
                } else {
                    rest.push(a.to_string());
                }
            }
        }
        i += 1;
    }
    let mut ms = None;
    if !ttl.is_empty() {
        match ttl_ms(&ttl) {
            Some(m) => ms = Some(m),
            None => {
                return Err(
                    ctx.usage("--ttl takes a duration: 30s, 5m, or milliseconds (at most 10m)")
                )
            }
        }
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("claim", &args)?;
    if action == "take" && lines.is_empty() {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc claim --lines a,b [--ttl 2m] [--note \"what for\"] [--hold] <file.md>"
        )));
    }
    let mut kv = vec![("action", w(action))];
    if !lines.is_empty() {
        kv.push(("lines", w(lines)));
    }
    if let Some(m) = ms {
        kv.push(("ttlMs", w(format!("@int:{m}"))));
    }
    if !note.is_empty() {
        kv.push(("note", w(note)));
    }
    if !hold.is_empty() {
        kv.push(("hold", w(hold)));
    }
    if !id.is_empty() {
        kv.push(("id", w(id)));
    }
    let body = ctx.body_arg(&path, &kv);
    let b = ctx.call(true, "/cli/docs/claim", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let a = gt(v, "action", 4)?.cloned();
        let p = doc_path(v, 4)?;
        if is_str(a.as_ref(), "release") {
            let r = gt_or(v, "released", 6)?;
            pr.line(&match r {
                Some(r) => format!("released {} on {p}", ops::join(r, ", ", 7)?),
                None => format!("you held no claim on {p}"),
            });
            return py::exit_int(gt(v, "exit", 8)?.unwrap_or(&Value::Int("0".into())), 8);
        }
        let c = ix(v, "claim", 9)?;
        let left = (num(ix(c, "remainingMs", 10)?, "/", "int", 10)? / 1000.0)
            .trunc()
            .max(0.0);
        let lines = ix(c, "lines", 11)?;
        let (l0, l1) = (st(&at(lines, 0, 11)?), st(&at(lines, 1, 11)?));
        let id = st(ix(c, "id", 11)?);
        let held = if tr(c, "hold", 12)? {
            " (held through your edits)"
        } else {
            " (your next edit there ends it)"
        };
        let waiting = match gt_or(c, "waiters", 13)? {
            Some(_) => format!(
                " · {} waiting",
                crate::families::py_len(ix(c, "waiters", 13)?, 13)?
            ),
            None => String::new(),
        };
        let verb = if is_str(a.as_ref(), "take") {
            "claimed"
        } else {
            "renewed"
        };
        pr.line(&format!(
            "{verb} lines {l0}-{l1} of {p} as {id} for {}s{held}{waiting}",
            left as i64
        ));
        Ok(Exit::Ok)
    }))
}

pub fn activity(ctx: &mut Ctx, words: &[String]) -> Step {
    let mut n = String::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if words[i] == "-n" {
            n = v2(words, i)?;
            i += 2;
            continue;
        }
        rest.push(words[i].clone());
        i += 1;
    }
    if !digits(&n) {
        return Err(ctx.usage("-n needs a number"));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("activity", &args)?;
    let q = ctx.query(&path, &[("n", &n)]);
    let b = ctx.call(false, "/cli/docs/activity", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        match gt_or(v, "rendered", 4)? {
            Some(r) => pr.put(&py::write_arg(r, 4)?),
            None => pr.put("No activity.\n"),
        }
        Ok(Exit::Ok)
    }))
}

/// `who` (`follow` `None`) and `follow <name>`.
pub fn who(ctx: &mut Ctx, follow: Option<&str>, args: &[String]) -> Step {
    let follow = follow.filter(|f| !f.is_empty());
    let path = match follow {
        Some(_) => {
            if args.len() != 2 {
                return Err(ctx.usage(format!("{CLI_NAME} doc follow <name> <file.md>")));
            }
            ctx.path(&args[1])
        }
        None => ctx.one_path("who", args)?,
    };
    let q = ctx.query(&path, &[]);
    let b = ctx.call(false, "/cli/docs/who", &q)?;
    let follow = follow.map(str::to_string);
    Ok(ctx.json_or(&b, |pr, v| {
        let rs = rows(v, "roster", 5)?;
        if let Some(f) = follow {
            let fl = f.to_lowercase();
            let mut hit = None;
            for r in &rs {
                let name = match gt_or(r, "name", 9)? {
                    Some(n) => py::text(n, "lower", 9)?.to_lowercase(),
                    None => String::new(),
                };
                if name.contains(&fl) || is_str(gt(r, "pid", 9)?, &f) {
                    hit = Some(r.clone());
                    break;
                }
            }
            let Some(r) = hit else {
                pr.eline(&format!("Error: {f} is not in this doc"));
                return Ok(Exit::NotFound);
            };
            let c = dict_or(&r, "cursor", 14)?;
            let vw = dict_or(&r, "view", 15)?;
            let head = gt_or(&c, "head", 16)?.map_or_else(|| "-".to_string(), st);
            let viewing = if opy::truthy(&vw) {
                format!("{}-{}", st(ix(&vw, "first", 17)?), st(ix(&vw, "last", 17)?))
            } else {
                gt_or(&r, "region", 17)?.map_or_else(|| "-".to_string(), st)
            };
            let lease = if tr(&r, "holdsLease", 18)? {
                "  holds the lease"
            } else {
                ""
            };
            pr.line(&format!(
                "{} {}  caret {head}  viewing {viewing}{lease}",
                sg(&r, "name", 16)?,
                sg(&r, "role", 16)?
            ));
            return Ok(Exit::Ok);
        }
        if rs.is_empty() {
            pr.line("Nobody is here.");
        }
        for r in &rs {
            let c = dict_or(r, "cursor", 23)?;
            let vw = dict_or(r, "view", 24)?;
            let tag = format!("{} ({})", sg(r, "name", 25)?, sg(r, "kind", 25)?);
            let mut extra: Vec<String> = Vec::new();
            if let Some(h) = gt_or(&c, "head", 27)? {
                extra.push(format!("caret {}", st(h)));
            }
            if opy::truthy(&vw) {
                extra.push(format!(
                    "viewing {}-{}",
                    st(ix(&vw, "first", 30)?),
                    st(ix(&vw, "last", 30)?)
                ));
            }
            if let Some(reg) = gt_or(r, "region", 31)? {
                extra.push(format!("viewing {}", st(reg)));
            }
            for cl in rows(r, "claims", 33)? {
                let lines = ix(&cl, "lines", 34)?;
                let note = match gt_or(&cl, "note", 35)? {
                    Some(_) => format!(" \"{}\"", st(ix(&cl, "note", 35)?)),
                    None => String::new(),
                };
                let secs = (num(ix(&cl, "remainingMs", 35)?, "/", "int", 35)? / 1000.0).trunc();
                let secs = d(&py::int_of_float(secs, 35)?, 34)?;
                extra.push(format!(
                    "claimed {}-{}{note} ({secs}s left)",
                    st(&at(lines, 0, 34)?),
                    st(&at(lines, 1, 34)?)
                ));
            }
            if gt_or(r, "lastEdit", 36)?.is_some() {
                let le = ix(r, "lastEdit", 37)?;
                let items = py::iter(le, 37)?;
                if items.len() != 2 {
                    let why = if items.len() < 2 {
                        "not enough arguments for format string"
                    } else {
                        "not all arguments converted during string formatting"
                    };
                    return Err(Raise::at(37, format!("TypeError: {why}")));
                }
                extra.push(format!(
                    "last edit at lines {}-{}",
                    st(&items[0]),
                    st(&items[1])
                ));
            }
            if tr(r, "holdsLease", 38)? {
                extra.push("holds the lease".into());
            }
            if tr(r, "away", 40)? {
                extra.push("away".into());
            }
            let conns = gt(r, "connections", 42)?
                .cloned()
                .unwrap_or(Value::Int("1".into()));
            if py::gt_int(&conns, 1, 42)? {
                extra.push(format!("{} connections", d(ix(r, "connections", 43)?, 43)?));
            }
            pr.line(&format!(
                "{} {} {}",
                crate::families::ljust(&tag, 32),
                left(gt(r, "role", 44)?, 7),
                extra.join("  ")
            ));
        }
        Ok(Exit::Ok)
    }))
}

pub fn say(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut to, mut at_, mut time, mut reply) =
        (String::new(), String::new(), String::new(), String::new());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        let (slot, hint) = match a {
            "--to" => (&mut to, "--to needs a person or agent"),
            "--at" => (&mut at_, "--at needs an address"),
            "--time" => (&mut time, "--time needs a time"),
            "--reply-to" => (&mut reply, "--reply-to needs a message id"),
            _ => {
                rest.push(a.to_string());
                i += 1;
                continue;
            }
        };
        match words.get(i + 1) {
            Some(v) => *slot = v.clone(),
            None => return Err(ctx.usage(hint)),
        }
        i += 2;
    }
    let args = ctx.parse(&rest)?;
    if args.is_empty() {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc say <file> [--to <who>] [--at <address>] \"text\""
        )));
    }
    let path = ctx.path(&args[0]);
    let msg = if args.len() >= 2 {
        args[1..].join(" ")
    } else if let Some(t) = &ctx.text {
        t.clone()
    } else {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc say <file> [--to <who>] \"text\": say what?"
        )));
    };
    ctx.text = Some(msg);
    let body = ctx.body_arg(
        &path,
        &[
            ("text", w("@text")),
            ("to", w(to)),
            ("at", w(at_)),
            ("time", w(time)),
            ("reply_to", w(reply)),
        ],
    );
    let b = ctx.call(true, "/cli/docs/say", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let v = ix(v, "say", 1)?;
        let id = st(ix(v, "id", 1)?);
        let to = if tr(v, "to", 1)? {
            match ix(v, "to", 1)? {
                t @ Value::Str(_) => format!(" to {}", st(t)),
                other => {
                    return Err(Raise::at(
                        1,
                        format!(
                            "TypeError: can only concatenate str (not \"{}\") to str",
                            opy::type_name(other)
                        ),
                    ))
                }
            }
        } else {
            " to the room".to_string()
        };
        pr.line(&format!("said {id}{to}"));
        Ok(Exit::Ok)
    }))
}

pub fn cursor(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut at_, mut to) = (String::new(), String::new());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        match words[i].as_str() {
            "--at" => {
                at_ = v2(words, i)?;
                i += 2;
            }
            "--to" | "--select" => {
                to = v2(words, i)?;
                i += 2;
            }
            a => {
                rest.push(a.to_string());
                i += 1;
            }
        }
    }
    if at_.is_empty() {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc cursor --at L:C [--to L:C] <file.md>"
        )));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("cursor", &args)?;
    let mut kv = vec![("at", w(at_))];
    if !to.is_empty() {
        kv.push(("to", w(to)));
    }
    let body = ctx.body_arg(&path, &kv);
    let b = ctx.call(true, "/cli/docs/cursor", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let c = ix(v, "cursor", 1)?;
        pr.line(&format!(
            "caret at bytes {}-{}",
            st(ix(c, "anchor", 1)?),
            st(ix(c, "head", 1)?)
        ));
        Ok(Exit::Ok)
    }))
}

/// `_doc_print_undo VERB`.
pub fn print_undo(ctx: &Ctx, verb: &str, body: &[u8]) -> Exit {
    if ctx.json {
        let mut b = body.to_vec();
        b.push(b'\n');
        if let Err(e) = crate::out::write_flushed(&mut std::io::stdout().lock(), &b) {
            return crate::out::after_stdout_error(&e);
        }
        let pr = Pr::default();
        let mut text = body.to_vec();
        text.push(b'\n');
        let r = py::load(&text, 1)
            .and_then(|v| py::exit_int(gt(&v, "exit", 1)?.unwrap_or(&Value::Int("0".into())), 1));
        return pr.finish(r);
    }
    let mut pr = Pr::default();
    let r = py::load(body, 3).and_then(|v| undo_lines(&mut pr, verb, &v));
    pr.finish(r)
}

fn undo_lines(pr: &mut Pr, verb: &str, v: &Value) -> R<Exit> {
    if tr(v, "nothing", 5)? {
        pr.eline(&format!("nothing to {verb}"));
    } else if tr(v, "historyLost", 7)? || tr(v, "clearedByPurge", 7)? {
        pr.eline(&sg_def(v, "hint", "no undo history", 8)?);
    } else if gt(v, "dropped", 9)?.is_some_and(|x| *x != Value::Null) {
        pr.eline(&format!(
            "dropped the top step (change {}); nothing applied",
            st(ix(v, "dropped", 10)?)
        ));
    } else {
        let r = dict_or(v, "result", 12)?;
        pr.eline(&format!(
            "{verb} {} step(s) (+{} -{}) · v{}",
            sg_def(v, "steps", "1", 13)?,
            sg_def(&r, "linesAdded", "0", 13)?,
            sg_def(&r, "linesRemoved", "0", 13)?,
            sg(&r, "version", 13)?
        ));
        for c in rows(v, "skipped", 14)? {
            let ln = gt_or(&c, "lines", 15)?
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![py::s("?"), py::s("?")]));
            pr.eline(&format!(
                "skipped lines {}-{} ({}): changed by someone since",
                st(&at(&ln, 0, 16)?),
                st(&at(&ln, 1, 16)?),
                sg(&c, "kind", 16)?
            ));
        }
        if tr(v, "blocked", 17)? {
            pr.eline("stopped: the next step overlaps later edits");
        }
    }
    py::exit_int(gt(v, "exit", 19)?.unwrap_or(&Value::Int("0".into())), 19)
}

pub fn undo_redo(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let (mut steps, mut mode, mut for_p) = ("1".to_string(), "all", String::new());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--steps" => {
                steps = v2(words, i)?;
                i += 2;
                continue;
            }
            "--for" => {
                for_p = v2(words, i)?;
                i += 2;
                continue;
            }
            "--partial" => mode = "partial",
            "--drop" => mode = "drop",
            _ => {
                if let Some(v) = eq(a, "--steps") {
                    steps = v.into();
                } else if let Some(v) = eq(a, "--for") {
                    for_p = v.into();
                } else {
                    rest.push(a.to_string());
                }
            }
        }
        i += 1;
    }
    if steps.is_empty() || !digits(&steps) {
        return Err(ctx.usage("--steps needs a number"));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path(verb, &args)?;
    let body = ctx.body_arg(
        &path,
        &[
            ("steps", w(format!("@int:{steps}"))),
            ("mode", w(mode)),
            ("forPerson", w(for_p)),
        ],
    );
    let b = ctx.call(true, &format!("/cli/docs/{verb}"), &body)?;
    Ok(print_undo(ctx, verb, &b))
}

/// `--since` in milliseconds before now.
fn since_ms(s: &str) -> Option<i64> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 || s.len() != n + 1 {
        return None;
    }
    let mult: f64 = match &s[n..] {
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604_800.0,
        _ => return None,
    };
    let x: f64 = s[..n].parse().ok()?;
    Some(((py::now_ms() / 1000.0 - x * mult) * 1000.0).trunc() as i64)
}

pub fn log(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut by, mut since, mut n, mut pend, mut for_p) = (
        String::new(),
        String::new(),
        String::new(),
        false,
        String::new(),
    );
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let slot = match words[i].as_str() {
            "--by" => &mut by,
            "--since" => &mut since,
            "-n" => &mut n,
            "--for" | "--on-behalf-of" => &mut for_p,
            "--pending" => {
                pend = true;
                i += 1;
                continue;
            }
            a => {
                rest.push(a.to_string());
                i += 1;
                continue;
            }
        };
        *slot = v2(words, i)?;
        i += 2;
    }
    if !digits(&n) {
        return Err(ctx.usage("-n needs a number"));
    }
    let mut sms = String::new();
    if !since.is_empty() {
        match since_ms(&since) {
            Some(x) => sms = x.to_string(),
            None => return Err(ctx.usage("--since takes a duration: 30m, 2h, 7d")),
        }
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("log", &args)?;
    let q = ctx.query(
        &path,
        &[
            ("by", &by),
            ("sinceMs", &sms),
            ("n", &n),
            ("pending", if pend { "1" } else { "0" }),
            ("forPerson", &for_p),
        ],
    );
    let b = ctx.call(false, "/cli/docs/log", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let rs = rows(v, "log", 4)?;
        if rs.is_empty() {
            pr.line("No history.");
        }
        for x in &rs {
            let at_ms = gt_or(x, "atMs", 8)?
                .cloned()
                .unwrap_or(Value::Int("0".into()));
            let t = local_ms(&at_ms, "%Y-%m-%d %H:%M:%S", 8)?;
            let on_behalf = |x: &Value| -> R<String> {
                Ok(format!(
                    " for {} ({})",
                    st(ix(x, "onBehalfOf", 13)?),
                    gt_or(x, "approval", 13)?.map_or_else(|| "pending".to_string(), st)
                ))
            };
            if is_str(gt(x, "type", 9)?, "version") {
                let name = match gt_or(x, "name", 10)? {
                    Some(_) => format!(" \"{}\"", st(ix(x, "name", 10)?)),
                    None => String::new(),
                };
                let mut by = match gt_or(x, "authorLabel", 11)? {
                    Some(_) => format!(
                        " by {} via {}, {} bytes",
                        st(ix(x, "authorLabel", 11)?),
                        sg(x, "via", 11)?,
                        sg(x, "size", 11)?
                    ),
                    None => String::new(),
                };
                if tr(x, "onBehalfOf", 12)? {
                    by += &on_behalf(x)?;
                }
                pr.line(&format!(
                    "  version {} {t}  {}{name}{by}",
                    left(gt(x, "id", 14)?, 5),
                    sg(x, "reason", 14)?
                ));
            } else {
                let mut who = match gt_or(x, "author", 16)? {
                    Some(a) => a.clone(),
                    None => py::s("?"),
                };
                if tr(x, "onBehalfOf", 17)? {
                    let Value::Str(_) = who else {
                        return Err(Raise::at(
                            18,
                            format!(
                                "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                                opy::type_name(&who)
                            ),
                        ));
                    };
                    who = py::s(&(st(&who) + &on_behalf(x)?));
                }
                let of = match gt(x, "of", 19)? {
                    Some(o) if *o != Value::Null => format!(" of {}", st(o)),
                    _ => String::new(),
                };
                pr.line(&format!(
                    "change {} {t}  {}{of} +{} -{}  {} via {}",
                    left(gt(x, "change", 20)?, 6),
                    left(gt(x, "kind", 20)?, 8),
                    sg_def(x, "linesAdded", "0", 21)?,
                    sg_def(x, "linesRemoved", "0", 21)?,
                    st(&who),
                    sg(x, "via", 21)?
                ));
            }
        }
        Ok(Exit::Ok)
    }))
}

fn mib(v: Option<&Value>, line: u32) -> R<String> {
    let b = v.cloned().unwrap_or(Value::Int("0".into()));
    let x = num(&b, "/", "int", line)? / 1_048_576.0;
    Ok(format!("{} MiB", py_f1(x)))
}

/// `"%.1f" % x`.
pub fn py_f1(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        format!("{x:.1}")
    }
}

pub fn status(ctx: &mut Ctx, words: &[String]) -> Step {
    let args = ctx.parse(words)?;
    if args.is_empty() {
        let b = ctx.call(false, "/cli/docs/status", "")?;
        return Ok(ctx.json_or(&b, pool_lines));
    }
    let path = ctx.one_path("status", &args)?;
    let q = ctx.query(&path, &[]);
    let b = ctx.call(false, "/cli/docs/status", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let dk = dict_or(v, "disk", 4)?;
        let r = dict_or(v, "rights", 5)?;
        let mut state = gt(&dk, "state", 6)?.cloned().unwrap_or(py::s("-"));
        if tr(&dk, "reason", 7)? {
            let Value::Str(_) = state else {
                return Err(Raise::at(
                    8,
                    format!(
                        "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                        opy::type_name(&state)
                    ),
                ));
            };
            state = py::s(&format!("{} ({})", st(&state), st(ix(&dk, "reason", 8)?)));
        }
        let tier0 = is_zero(gt(v, "tier", 9)?);
        let kind = if tier0 { " · any file (tier 0)" } else { "" };
        pr.line(&format!(
            "{}{kind} · v{} · {} here ({} connections) · disk {}",
            doc_path(v, 10)?,
            sg(v, "version", 10)?,
            sg(v, "participants", 10)?,
            sg(v, "connections", 10)?,
            st(&state)
        ));
        if tier0 {
            let c = dict_or(v, "current", 13)?;
            let at_ms = gt_or(&c, "atMs", 15)?
                .cloned()
                .unwrap_or(Value::Int("0".into()));
            let author = gt_or(&c, "authorLabel", 14)?.or(gt(&c, "author", 14)?);
            pr.line(&format!(
                "current: {} bytes, {} by {} via {} at {}",
                sg(&c, "size", 14)?,
                sg(&c, "reason", 14)?,
                so(author),
                sg(&c, "via", 15)?,
                local_ms(&at_ms, "%Y-%m-%d %H:%M:%S", 15)?
            ));
            match gt_or(v, "lease", 16)? {
                Some(l) => {
                    let label = sg(l, "label", 18)?;
                    let until = local_ms(ix(l, "expiresMs", 18)?, "%H:%M:%S", 18)?;
                    let mine = if tr(l, "mine", 19)? { " (yours)" } else { "" };
                    pr.line(&format!("lease: {label} until {until}{mine}"));
                }
                None => pr.line("lease: free"),
            }
            for cp in rows(v, "conflictCopies", 22)? {
                pr.line(&format!(
                    "conflict copy: {} (version {}, kept for 30 days)",
                    sg(&cp, "path", 23)?,
                    sg(&cp, "kept", 23)?
                ));
            }
        }
        let mut have = Vec::new();
        for k in ["read", "edit", "history"] {
            if tr(&r, k, 24)? {
                have.push(k);
            }
        }
        pr.line(&format!("your rights: {}", have.join(", ")));
        if tr(v, "declined", 25)? {
            pr.line(&format!(
                "live editing is off: {}",
                st(ix(v, "declined", 26)?)
            ));
        }
        Ok(Exit::Ok)
    }))
}

fn pool_lines(pr: &mut Pr, v: &Value) -> R<Exit> {
    let p = dict_or(v, "pool", 3)?;
    if !tr(&p, "running", 4)? {
        pr.line(&format!(
            "live docs: off ({})",
            sg_def(&p, "error", "?", 5)?
        ));
        return Ok(Exit::Ok);
    }
    pr.line(&format!(
        "live docs: {} loaded · {} of {} · {} unloaded since start · disk sync {}",
        sg(&p, "loaded", 9)?,
        mib(gt(&p, "estimatedBytes", 9)?, 9)?,
        mib(gt(&p, "budgetBytes", 9)?, 9)?,
        sg(&p, "unloadedTotal", 9)?,
        if tr(&p, "diskSync", 10)? { "on" } else { "off" }
    ));
    let a = dict_or(&p, "anyFile", 11)?;
    if tr(&a, "on", 12)? {
        pr.line(&format!(
            "any-file docs: {} loaded · {} blobs, {} ({} unreferenced) · files up to {}",
            sg(&a, "loaded", 13)?,
            sg(&a, "blobs", 13)?,
            mib(gt(&a, "bytes", 13)?, 13)?,
            mib(gt(&a, "unreferencedBytes", 13)?, 13)?,
            mib(gt(&a, "maxFileBytes", 13)?, 13)?
        ));
    } else if opy::truthy(&a) {
        pr.line("any-file docs: off (see the daemon log)");
    }
    let sv = dict_or(&p, "saves", 16)?;
    if opy::truthy(&sv) {
        let how = if is_str(gt(&sv, "commits", 18)?, "grouped") {
            format!(
                "grouped ({} changes or {} ms per wait for the disk)",
                sg(&sv, "groupMaxChanges", 18)?,
                sg(&sv, "groupMaxMs", 18)?
            )
        } else {
            "every change at once".to_string()
        };
        let restart = if tr(&sv, "restartNeeded", 20)? {
            format!(" · restart {PRODUCT_NAME} for Save every change immediately to take effect")
        } else {
            String::new()
        };
        pr.line(&format!(
            "saves: {how} · {} of {} on disk{restart}",
            sg(&sv, "commitsOnDisk", 19)?,
            sg(&sv, "commitsMade", 19)?
        ));
    }
    Ok(Exit::Ok)
}

pub fn flush_open_leave(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let args = ctx.parse(words)?;
    let path = ctx.one_path(verb, &args)?;
    let body = ctx.body_arg(&path, &[]);
    let b = ctx.call(true, &format!("/cli/docs/{verb}"), &body)?;
    let verb = verb.to_string();
    Ok(ctx.json_or(&b, |pr, v| {
        match verb.as_str() {
            "flush" => {
                let p = doc_path(v, 6)?;
                let dk = dict_or(v, "disk", 6)?;
                let written = if tr(v, "written", 6)? {
                    " (written)"
                } else {
                    ""
                };
                pr.line(&format!(
                    "{p} · disk {}{written}",
                    sg_def(&dk, "state", "-", 6)?
                ));
            }
            "open" => pr.line(&format!(
                "in {} · v{} (stay present until {CLI_NAME} doc leave)",
                doc_path(v, 8)?,
                sg(v, "version", 8)?
            )),
            _ => pr.line(&format!("left {}", doc_path(v, 10)?)),
        }
        Ok(Exit::Ok)
    }))
}

pub fn approvals(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut pend, mut mine, mut for_p) = ("0", "0", String::new());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--pending" => pend = "1",
            "--mine" | "--on-behalf-of-me" => mine = "1",
            "--for" | "--on-behalf-of" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(ctx.usage(format!("{a} needs a person (or me)")));
                };
                for_p = v.clone();
                i += 2;
                continue;
            }
            _ => rest.push(a.to_string()),
        }
        i += 1;
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("approvals", &args)?;
    let q = ctx.query(
        &path,
        &[("pending", pend), ("mine", mine), ("forPerson", &for_p)],
    );
    let b = ctx.call(false, "/cli/docs/approvals", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let rs = rows(v, "approvals", 3)?;
        if rs.is_empty() {
            pr.line("No approvals.");
        }
        for r in &rs {
            let a = dict_or(r, "approval", 7)?;
            let by = match gt_or(&a, "by", 9)? {
                Some(_) => format!(" (settled by {})", st(ix(&a, "by", 9)?)),
                None => String::new(),
            };
            pr.line(&format!(
                "change {} {} by {} for {}{by}",
                left(gt(r, "change", 8)?, 6),
                left(gt(&a, "state", 8)?, 9),
                sg(&a, "author", 8)?,
                sg(&a, "onBehalfOf", 8)?
            ));
        }
        Ok(Exit::Ok)
    }))
}

pub fn approve(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let args = ctx.parse(words)?;
    if args.len() != 2 {
        return Err(ctx.usage(format!("{CLI_NAME} doc {verb} <change> <file.md>")));
    }
    let change = args[0].clone();
    if change.is_empty() || !digits(&change) {
        return Err(ctx.usage(format!("the change is a number ({CLI_NAME} doc approvals)")));
    }
    let path = ctx.path(&args[1]);
    let state = if verb == "decline" {
        "declined"
    } else {
        "approved"
    };
    let body = ctx.body_arg(
        &path,
        &[("change", w(format!("@int:{change}"))), ("state", w(state))],
    );
    let b = ctx.call(true, "/cli/docs/approve", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let what = if is_zero(gt(v, "tier", 4)?) {
            "version"
        } else {
            "change"
        };
        let ch = sg(v, "change", 4)?;
        let state = st(ix(ix(v, "approval", 4)?, "state", 4)?);
        pr.line(&format!("{what} {ch} {state}"));
        if tr(v, "hint", 5)? {
            pr.eline(&st(ix(v, "hint", 6)?));
        }
        Ok(Exit::Ok)
    }))
}

pub fn share(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let mut with = String::new();
    let (mut wr, mut hi, mut co) = ("@false", "@false", "@false");
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--with" => {
                with = v2(words, i)?;
                i += 2;
                continue;
            }
            "--write" => wr = "@true",
            "--history" => hi = "@true",
            "--comment" => co = "@true",
            _ => match eq(a, "--with") {
                Some(v) => with = v.into(),
                None => rest.push(a.to_string()),
            },
        }
        i += 1;
    }
    let args = ctx.parse(&rest)?;
    if with.is_empty() {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc {verb} <file-or-folder> --with <workspace> -w <its workspace>"
        )));
    }
    if ctx.ws.is_empty() {
        return Err(ctx.usage("name the workspace the file is in: -w <workspace>"));
    }
    if args.len() != 1 {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc {verb} <file-or-folder> --with <workspace> -w <ws>"
        )));
    }
    let rel = args[0].clone();
    let b = if verb == "share" {
        let body = ctx.body_arg(
            &rel,
            &[
                ("with", w(with)),
                ("write", w(wr)),
                ("history", w(hi)),
                ("comment", w(co)),
            ],
        );
        ctx.call(true, "/cli/docs/share", &body)?
    } else {
        let body = ctx.body_arg(&rel, &[("with", w(with))]);
        ctx.call(true, "/cli/docs/unshare", &body)?
    };
    Ok(ctx.json_or(&b, |pr, v| {
        let sh = match gt_or(v, "share", 4)? {
            Some(x) => x.clone(),
            None => dict_or(v, "removed", 4)?,
        };
        let g = grants(&sh, 5)?;
        let shared = tr(v, "share", 6)?;
        let path = gt_or(&sh, "path", 7)?.map_or_else(|| ".".to_string(), st);
        let kind = if tr(&sh, "folder", 7)? {
            "folder"
        } else {
            "file"
        };
        let what = if shared { "shared" } else { "unshared" };
        pr.line(&format!(
            "{what} {path} ({kind}) · {}",
            if shared { g.as_str() } else { "" }
        ));
        Ok(Exit::Ok)
    }))
}

pub fn shares(ctx: &mut Ctx, words: &[String]) -> Step {
    let mut with = String::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if words[i] == "--with" {
            with = v2(words, i)?;
            i += 2;
            continue;
        }
        rest.push(words[i].clone());
        i += 1;
    }
    ctx.parse(&rest)?;
    let q = ctx.query("", &[("with", &with)]);
    let b = ctx.call(false, "/cli/docs/shares", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let rs = rows(v, "shares", 3)?;
        if rs.is_empty() {
            pr.line("No shares.");
        }
        for sh in &rs {
            let g = grants(sh, 7)?;
            pr.line(&format!(
                "{}:{}{} -> {}  ({g})",
                sg(sh, "owner", 8)?,
                gt_or(sh, "path", 8)?.map_or_else(|| ".".to_string(), st),
                if tr(sh, "folder", 8)? { "/" } else { "" },
                sg(sh, "sharee", 8)?
            ));
        }
        Ok(Exit::Ok)
    }))
}

pub fn blame(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut range, mut form) = (String::new(), "rendered");
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        if a == "-L" {
            range = v2(words, i)?;
            i += 2;
            continue;
        }
        if let Some(r) = a.strip_prefix("-L") {
            range = r.to_string();
        } else if a == "--porcelain" || a == "--line-porcelain" {
            form = "porcelain";
        } else {
            rest.push(a.to_string());
        }
        i += 1;
    }
    let ok = range.is_empty()
        || (range.as_bytes()[0].is_ascii_digit()
            && range.bytes().all(|b| b.is_ascii_digit() || b == b','));
    if !ok {
        return Err(ctx.usage("-L takes a,b (line numbers)"));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("blame", &args)?;
    let q = ctx.query(&path, &[("lines", &range)]);
    let b = ctx.call(false, "/cli/docs/blame", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let x = match gt_or(v, form, 1)? {
            Some(x) => Some(x),
            None => gt_or(v, "rendered", 1)?,
        };
        if let Some(x) = x {
            pr.put(&py::write_arg(x, 1)?);
        }
        Ok(Exit::Ok)
    }))
}

/// The shell's `dirname`.
fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.starts_with('/') {
            "/".into()
        } else {
            ".".into()
        };
    }
    match t.rfind('/') {
        None => ".".into(),
        Some(i) => {
            let h = t[..i].trim_end_matches('/');
            if h.is_empty() {
                "/".into()
            } else {
                h.into()
            }
        }
    }
}

pub fn mv(ctx: &mut Ctx, words: &[String]) -> Step {
    let args = ctx.parse(words)?;
    if args.len() != 2 {
        return Err(ctx.usage(format!("{CLI_NAME} doc mv <file.md> <new-name.md>")));
    }
    let path = ctx.path(&args[0]);
    let mut to = args[1].clone();
    let tcp = ctx.client.as_ref().is_some_and(|c| c.force_tcp);
    if ctx.ws.is_empty() && !tcp && std::path::Path::new(&dirname(&to)).is_dir() {
        to = pypath::abspath(&to);
    }
    let body = ctx.body_arg(&path, &[("to", w(to))]);
    let b = ctx.call(true, "/cli/docs/mv", &body)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let from = sg(v, "from", 1)?;
        pr.line(&format!("moved {from} -> {}", doc_path(v, 1)?));
        Ok(Exit::Ok)
    }))
}

/// The key of a not-reached entry's command, in the daemon's answer.
const NOT_REACHED_HOW: &str = "k2";

pub fn owner_purge(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let (mut thread, mut n, mut text, mut all, mut who, mut pseudo) = (
        String::new(),
        String::new(),
        String::new(),
        false,
        String::new(),
        String::new(),
    );
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let slot = match words[i].as_str() {
            "--thread" => &mut thread,
            "--n" => &mut n,
            "--text" => &mut text,
            "--participant" => &mut who,
            "--pseudonym" => &mut pseudo,
            "--all" => {
                all = true;
                i += 1;
                continue;
            }
            a => {
                rest.push(a.to_string());
                i += 1;
                continue;
            }
        };
        *slot = v2(words, i)?;
        i += 2;
    }
    if !digits(&n) {
        return Err(ctx.usage("--n takes a comment number"));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path(verb, &args)?;
    let b = if verb == "purge-comments" {
        let mut kv: Vec<(&str, B)> = Vec::new();
        if !thread.is_empty() {
            kv.push(("thread", w(thread)));
        }
        if !n.is_empty() {
            kv.push(("n", w(format!("@int:{n}"))));
        }
        if !text.is_empty() {
            kv.push(("text", w(text)));
        }
        if all {
            kv.push(("all", w("@true")));
        }
        if kv.is_empty() {
            return Err(ctx.usage(format!(
                "{CLI_NAME} doc purge-comments --thread ann-N [--n N] | --text \"...\" | --all <file>"
            )));
        }
        let body = ctx.body_arg(&path, &kv);
        ctx.call(true, "/cli/docs/comments/purge", &body)?
    } else {
        if who.is_empty() || pseudo.is_empty() {
            return Err(ctx.usage(format!(
                "{CLI_NAME} doc erase-participant --participant P --pseudonym NAME <file>"
            )));
        }
        let body = ctx.body_arg(&path, &[("participant", w(who)), ("pseudonym", w(pseudo))]);
        ctx.call(true, "/cli/docs/erase", &body)?
    };
    Ok(ctx.json_or(&b, |pr, v| {
        let c = dict_or(v, "counts", 4)?;
        if tr(v, "erased", 5)? {
            pr.line(&format!(
                "erased {} as {} from {} ({} attributions, {} comments cleared)",
                st(ix(v, "erased", 6)?),
                sg(v, "pseudonym", 6)?,
                doc_path(v, 6)?,
                sg_def(&c, "attributions", "0", 6)?,
                sg_def(&c, "comments", "0", 6)?
            ));
        } else {
            pr.line(&format!(
                "purged {} comment(s) and {} pin copies from {}",
                sg_def(&c, "comments", "0", 8)?,
                sg_def(&c, "anchors", "0", 8)?,
                doc_path(v, 8)?
            ));
        }
        pr.eline(if tr(v, "storeRewritten", 9)? {
            "store file rewritten"
        } else {
            "the store file was NOT rewritten: purged bytes stay in freed pages until the next rewrite"
        });
        for nr in rows(v, "notReached", 10)? {
            let k2 = match gt_or(&nr, NOT_REACHED_HOW, 11)? {
                Some(_) => match ix(&nr, NOT_REACHED_HOW, 11)? {
                    x @ Value::Str(_) => format!(" ({})", st(x)),
                    other => {
                        return Err(Raise::at(
                            11,
                            format!(
                                "TypeError: can only concatenate str (not \"{}\") to str",
                                opy::type_name(other)
                            ),
                        ))
                    }
                },
                None => String::new(),
            };
            pr.eline(&format!("not reached: {}{k2}", sg(&nr, "describe", 11)?));
        }
        Ok(Exit::Ok)
    }))
}
