//! `k2 vault systemd-cred NAME --unit U [--cred-name C] [--restart]
//! [--dry-run] | --remove | --replace` (`_vault_systemd_cred`,
//! `cli/src/job.sh`): a secret sealed into a systemd credential a root
//! unit loads. People only (the vault socket and a confirm bound to the
//! plan), Linux only. The value never passes through here: the daemon
//! resolves it and pipes it to the root helper.

use super::py::{self, get, idx, is, items, or_list, or_obj, render, s, t, Lines, R};
use super::verbs::{echo, kv, merge, obj, print_body, scope_json, usage};
use super::Ctx;
use crate::confirm::{self, Split};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out;
use crate::out::json::{self, Value};
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};

/// `x == 0` in Python: `0`, `0.0` and `False`.
fn is_zero(v: &Value) -> bool {
    match v {
        Value::Int(d) => d == "0",
        Value::Float(f) => *f == 0.0,
        Value::Bool(b) => !b,
        _ => false,
    }
}

/// `key in v`: a dict's keys, a list's items, a string's substrings.
fn has(v: &Value, key: &str) -> R<bool> {
    match v {
        Value::Object(pairs) => Ok(pairs.iter().any(|(k, _)| k.as_str() == Some(key))),
        Value::Array(xs) => Ok(xs.iter().any(|x| is(x, key))),
        Value::Str(_) => Ok(s(v).contains(key)),
        other => Err(Raise::at(
            1,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                crate::out::py::type_name(other)
            ),
        )),
    }
}

/// `printf '%s' "$_SC_BODY" | python3 -c '…sys.exit(0 if <test> else 1)'`
/// as an `if` reads it: a raise prints its traceback and is false.
fn py_test(body: &[u8], f: impl FnOnce(&Value) -> R<bool>) -> bool {
    match confirm::load(body).and_then(|v| f(&v)) {
        Ok(b) => b,
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            false
        }
    }
}

/// `systemd-cred`'s options.
#[derive(Default)]
struct Opts {
    name: String,
    unit: String,
    cred: String,
    scope: String,
    restart: bool,
    dry: bool,
    remove: bool,
    replace: bool,
    json: bool,
    accept: bool,
}

fn parse(rest: &[&str]) -> Result<Opts, Exit> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--accept-agent-value" => o.accept = true,
            "--unit" | "--cred-name" | "--scope" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    return Err(usage(format!("usage: {w} needs a value")));
                }
                match w {
                    "--unit" => o.unit = v.into(),
                    "--cred-name" => o.cred = v.into(),
                    _ => o.scope = v.into(),
                }
                i += 1;
            }
            "--restart" => o.restart = true,
            "--dry-run" => o.dry = true,
            "--remove" => o.remove = true,
            "--replace" => o.replace = true,
            "--json" => o.json = true,
            _ if w == "--value" || w.starts_with("--value=") || w.starts_with("--secret-value") => {
                return Err(usage(format!(
                    "value_on_argv: {PRODUCT} takes the value from the vault, never an argument"
                )));
            }
            "--system" => {
                return Err(usage(format!(
                    "usage: only {CLI} vault list takes --system"
                )))
            }
            _ if w.starts_with("--") => {
                return Err(usage(format!(
                    "usage: unknown flag '{w}' ({CLI} vault systemd-cred --help)"
                )))
            }
            _ => {
                if !o.name.is_empty() {
                    return Err(usage(format!("usage: {CLI} vault systemd-cred NAME --unit <unit.service> [--restart] [--dry-run] | --remove | --replace")));
                }
                o.name = w.into();
            }
        }
        i += 1;
    }
    Ok(o)
}

/// The verb.
pub fn run(ctx: &mut Ctx, rest: &[&str]) -> Result<Exit, Exit> {
    let o = parse(rest)?;
    let line = format!("usage: {CLI} vault systemd-cred NAME --unit <unit.service> [--cred-name <id>] [--restart] [--dry-run] | NAME --unit <unit> --remove | NAME --replace");
    if o.name.is_empty() {
        return Err(usage(line));
    }
    if o.replace {
        if !(o.unit.is_empty() && !o.remove && !o.restart && o.cred.is_empty()) {
            return Err(usage("usage: --replace takes no --unit, --cred-name, --restart or --remove (each placement keeps its own)".into()));
        }
    } else if o.unit.is_empty() {
        return Err(usage(line));
    }
    if o.remove && o.restart {
        return Err(usage(
            "usage: --remove takes no --restart (restart the unit yourself after)".into(),
        ));
    }
    if std::env::consts::OS != "linux" {
        return Err(usage(format!("linux_only: systemd credentials are Linux only (systemd 250+); on this machine use {CLI} vault fetch, file or env delivery")));
    }
    ctx.person_guard()?;
    let (op, route) = if o.remove {
        ("remove", "systemd-cred/remove")
    } else if o.replace {
        ("replace", "systemd-cred")
    } else {
        ("place", "systemd-cred")
    };
    let sj = scope_json(&o.scope)?;
    let nj = vec![kv("name", py::text(&o.name))];
    let mut b = vec![kv("op", py::text(op))];
    if !o.unit.is_empty() {
        b.push(kv("unit", py::text(&o.unit)));
    }
    if !o.cred.is_empty() {
        b.push(kv("credName", py::text(&o.cred)));
    }
    if o.restart {
        b.push(kv("restart", Value::Bool(true)));
    }
    if op == "replace" {
        b.push(kv("replace", Value::Bool(true)));
    }
    let body = merge(&[&sj, &nj, &b]);
    // Everything checked first (unit, allow list, ceiling, helper), with
    // no confirm; the daemon words what the person is about to approve.
    let dry = merge(&[&body, &[kv("dryRun", Value::Bool(true))]]);
    let d = ctx.sock(true, route, obj(dry).as_bytes());
    if !d.ok() {
        return Err(ctx.fail(&d));
    }
    if o.dry {
        if o.json {
            return Ok(print_body(&d));
        }
        render(&d.body, dry_printer).print();
        return Ok(Exit::Ok);
    }
    let text = match confirm::load(&d.body).and_then(|v| get(&v, "confirmText")) {
        Ok(x) if t(&x) => s(&x).trim_end_matches('\n').to_string(),
        Ok(_) => String::new(),
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            String::new()
        }
    };
    let mut body = obj(body);
    if op != "remove" {
        // Bound to what the dry run showed: this exact version (and, for
        // --replace, these placements). A value no person wrote needs
        // --accept-agent-value, after a look at it.
        let person_wrote = py_test(&d.body, |v| {
            Ok(t(&py::get_or(v, "authorIsPerson", Value::Bool(true))?)
                || is_zero(&get(v, "behind")?))
        });
        if !person_wrote && !o.accept {
            out::stderr_line(&text);
            out::stderr_line(&format!("agent_written_value: no person wrote this value and it would run in a root unit. Review it first ({CLI} vault reveal {}), then add --accept-agent-value.", o.name));
            return Err(Exit::Refused);
        }
        body = match bound(&d.body, &body, o.accept) {
            Ok(b) => b,
            Err(e) => {
                out::stderr_line(e.text().trim_end_matches('\n'));
                String::new()
            }
        };
    }
    if op == "replace" && py_test(&d.body, |v| Ok(is_zero(&get(v, "behind")?))) {
        return Ok(echo(&text));
    }
    if op != "remove" && !py_test(&d.body, |v| Ok(t(&get(v, "helperReady")?))) {
        let r = ctx.sock(true, route, body.as_bytes());
        return Err(ctx.fail(&r));
    }
    let Some(tok) =
        confirm::confirm_token_plan(&mut ctx.confirm(), "systemd-cred", &body, "systemdCred")
    else {
        return Err(Exit::Refused);
    };
    let sent = confirm::with_confirm_token(&body, &tok, None).unwrap_or_default();
    let r: Split = ctx.sock(true, route, sent.as_bytes());
    if !r.ok() {
        return Err(ctx.fail(&r));
    }
    if o.json {
        return Ok(print_body(&r));
    }
    Ok(render(&r.body, done_printer).print())
}

/// The plan bound to the dry run's answer: its `version`, its `rows` when
/// it has them, and `acceptAgentValue`.
fn bound(answer: &[u8], body: &str, accept: bool) -> R<String> {
    let d = confirm::load(answer)?;
    let version = get(&d, "version")?;
    let Ok(Value::Object(mut o)) = json::parse(body) else {
        return Ok(String::new());
    };
    let mut set = |k: &str, v: Value| match o.iter_mut().find(|(ok, _)| ok.as_str() == Some(k)) {
        Some((_, old)) => *old = v,
        None => o.push(kv(k, v)),
    };
    set("version", version);
    if has(&d, "rows")? {
        set("rows", idx(&d, "rows")?);
    }
    if accept {
        set("acceptAgentValue", Value::Bool(true));
    }
    Ok(obj(o))
}

fn dry_printer(v: &Value, l: &mut Lines) -> R<i32> {
    let p = or_obj(get(v, "plan")?);
    l.p(format!("would: {}", s(&get(v, "confirmText")?)));
    if t(&p) {
        l.p(format!("file:      {}", s(&get(&p, "credPath")?)));
        l.p(format!("drop-in:   {}", s(&get(&p, "dropin")?)));
        l.p(format!("           {}", s(&get(&p, "dropinLine")?)));
        l.p(format!("reads it:  {}", s(&get(&p, "readAs")?)));
    }
    if get(v, "allowed")? == Value::Bool(false) {
        l.p(format!(
            "the unit is not on this box allow list yet ({CLI} vault systemd-cred --help)"
        ));
    }
    if get(v, "helperReady")? == Value::Bool(false) {
        l.p(format!(
            "the root helper must be updated first: {CLI} root-helper update"
        ));
    }
    Ok(0)
}

fn done_printer(v: &Value, l: &mut Lines) -> R<i32> {
    if has(v, "credRemoved")? {
        let stays = if t(&get(v, "credRemoved")?) {
            ""
        } else {
            " (the .cred stays: another unit loads it)"
        };
        l.p(format!(
            "took the credential {} off {}{stays}",
            s(&get(v, "credName")?),
            s(&get(v, "unit")?)
        ));
    } else if has(v, "replaced")? {
        for r in items(&or_list(get(v, "replaced")?))? {
            if t(&get(&r, "ok")?) {
                let tail = if t(&get(&r, "restarted")?) {
                    ", restart queued".to_string()
                } else if t(&get(&r, "restartError")?) {
                    format!(" (restart refused: {})", s(&idx(&r, "restartError")?))
                } else {
                    String::new()
                };
                l.p(format!(
                    "re-placed v{} into {}{tail}",
                    s(&get(&r, "version")?),
                    s(&get(&r, "unit")?)
                ));
            } else {
                let unit = get(&r, "unit")?;
                let e = or_obj(get(&r, "error")?);
                l.p(format!(
                    "NOT re-placed into {}: {}: {}",
                    s(&unit),
                    s(&get(&e, "code")?),
                    s(&get(&e, "hint")?)
                ));
            }
        }
        if !t(&get(v, "replaced")?) {
            l.p("nothing was behind");
        }
    } else {
        let p = or_obj(get(v, "plan")?);
        let restarted = if t(&get(v, "restarted")?) {
            ", restart queued"
        } else {
            ""
        };
        l.p(format!(
            "sealed {} v{} into {}, bound to {}{restarted}",
            s(&get(&p, "secret")?),
            s(&get(v, "version")?),
            s(&get(&p, "credPath")?),
            s(&get(&p, "unit")?)
        ));
        l.p(format!("the service reads {}", s(&get(&p, "readAs")?)));
    }
    if t(&get(v, "next")?) {
        l.p(s(&idx(v, "next")?));
    }
    Ok(if t(&py::get_or(v, "ok", Value::Bool(true))?) {
        0
    } else {
        1
    })
}
