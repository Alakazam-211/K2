//! The vault verbs with parsers of their own: sync (`_vault_sync`,
//! `cli/src/vault.sh`) and placement and placements (`_vault_placement`,
//! `cli/src/job.sh`). fetch (`_vault_fetch`), run (`_vault_run`) and
//! systemd-cred (`_vault_systemd_cred`) are the shell's still.

use super::py::{self, get, idx, items, left, or_list, render, s, t, Lines, R};
use super::verbs::{merge, scope_json};
use super::Ctx;
use crate::confirm::dumps;
use crate::exit::Exit;
use crate::families::Shown;
use crate::out;
use crate::out::json::{Text, Value};
use k2_client::brand::CLI_NAME as CLI;

/// Whether the binary answers `vault <sub> <rest>` (no I/O).
pub fn native(sub: &str, rest: &[&str]) -> bool {
    match sub {
        "sync" | "placements" => true,
        "placement" => matches!(
            rest.first().copied().unwrap_or("list"),
            "add" | "done" | "rm" | "remove" | "list"
        ),
        _ => false,
    }
}

fn usage(line: String) -> Exit {
    out::stderr_line(&line);
    Exit::Usage
}

/// The verb. Called only for a form [`native`] takes.
pub fn run(ctx: &mut Ctx, sub: &str, rest: &[&str]) -> Exit {
    let r = match sub {
        "sync" => sync(ctx, rest),
        _ => placement(ctx, sub, rest),
    };
    match r {
        Ok(e) | Err(e) => e,
    }
}

fn print_body(body: &[u8]) -> Exit {
    let mut b = body.to_vec();
    b.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &b) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `_vault_sync`.
fn sync(ctx: &mut Ctx, rest: &[&str]) -> Result<Exit, Exit> {
    let mut json = false;
    for w in rest {
        match *w {
            "--json" => json = true,
            "--system" => {
                return Err(usage(format!(
                    "usage: only {CLI} vault list takes --system"
                )))
            }
            _ => return Err(usage(format!("usage: {CLI} vault sync [--json]"))),
        }
    }
    let r = ctx.call(true, "sync", "{}")?;
    if json {
        return Ok(print_body(&r.body));
    }
    Ok(render(&r.body, |v, l| {
        if t(&get(v, "dir")?) {
            l.p(format!("secrets dir: {}", s(&idx(v, "dir")?)));
        }
        for (key, word) in [
            ("delivered", "delivered  "),
            ("present", "present    "),
            ("restart", "restart    "),
            ("skipped", "skipped    "),
        ] {
            for f in items(&or_list(get(v, key)?))? {
                l.p(format!("{word}{}", s(&f)));
            }
        }
        let h = get(v, "hint")?;
        l.p(if t(&h) { s(&h) } else { String::new() });
        Ok(if t(&get(v, "skipped")?) { 1 } else { 0 })
    })
    .print())
}

/// `_vault_placement <sub> …`.
fn placement(ctx: &mut Ctx, sub: &str, rest: &[&str]) -> Result<Exit, Exit> {
    let (act, args) = if sub == "placements" {
        ("list", rest)
    } else {
        match rest.split_first() {
            Some((a, r)) => (*a, r),
            None => ("list", rest),
        }
    };
    let (mut name, mut where_, mut note, mut scope, mut json) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        false,
    );
    let mut seen: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let w = args[i];
        match w {
            "--where" | "--note" | "--scope" => {
                let v = args.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    return Err(usage(format!("usage: {w} needs a value")));
                }
                match w {
                    "--where" => where_ = v.into(),
                    "--note" => note = v.into(),
                    _ => scope = v.into(),
                }
                seen.push(w);
                i += 2;
                continue;
            }
            "--json" => {
                json = true;
                seen.push(w);
            }
            "--system" => {
                return Err(usage(format!(
                    "usage: only {CLI} vault list takes --system"
                )))
            }
            _ if w.starts_with("--") => {
                return Err(usage(format!(
                    "usage: unknown flag '{w}' ({CLI} vault placement --help)"
                )))
            }
            _ => {
                if !name.is_empty() {
                    return Err(usage(format!("usage: {CLI} vault placement --help")));
                }
                name = w.into();
            }
        }
        i += 1;
    }
    let (takes, words) = match act {
        "add" => ("--where --note --scope --json", format!("placement {act}")),
        "list" if sub == "placements" => ("--scope --json", "placements".to_string()),
        "list" => ("--scope --json", format!("placement {act}")),
        _ => ("--json", format!("placement {act}")),
    };
    for f in &seen {
        if !format!(" {takes} ").contains(&format!(" {f} ")) {
            return Err(usage(format!(
                "usage: {CLI} vault {words} doesn't take {f} (it takes: {}; {CLI} vault placement --help)",
                takes.replace(' ', ", ")
            )));
        }
    }
    if act == "list" && !scope.is_empty() && name.is_empty() {
        return Err(usage(format!(
            "usage: --scope picks which secret NAME is: {CLI} vault placements NAME --scope <s>"
        )));
    }
    let sj = scope_json(&scope)?;
    let kv = |k: &str, v: &str| (Text::from(k), py::text(v));
    match act {
        "add" => {
            if name.is_empty() || where_.is_empty() {
                return Err(usage(format!(
                    "usage: {CLI} vault placement add NAME --where <host>:<path> [--note …]"
                )));
            }
            let mut b = merge(&[&sj, &[kv("name", &name), kv("where", &where_)]]);
            if !note.is_empty() {
                b.push(kv("note", &note));
            }
            let r = ctx.call(true, "placement/add", &dumps(&Value::Object(b)))?;
            if json {
                return Ok(print_body(&r.body));
            }
            Ok(render(&r.body, |v, l| {
                let p = idx(v, "placement")?;
                l.p(format!(
                    "noted {} at {} (v{}): {}",
                    s(&get(&p, "name")?),
                    s(&get(&p, "where")?),
                    s(&get(&p, "version")?),
                    s(&get(&p, "id")?)
                ));
                Ok(0)
            })
            .print())
        }
        "done" | "rm" | "remove" => {
            if name.is_empty() {
                return Err(usage(format!(
                    "usage: {CLI} vault placement {act} ID (ids: {CLI} vault placements)"
                )));
            }
            let route = if act == "done" {
                "placement/done"
            } else {
                "placement/remove"
            };
            let r = ctx.call(true, route, &dumps(&Value::Object(vec![kv("id", &name)])))?;
            if json {
                return Ok(print_body(&r.body));
            }
            if act == "done" {
                Ok(render(&r.body, |v, l| {
                    let p = idx(v, "placement")?;
                    l.p(format!(
                        "{} at {} now holds v{}",
                        s(&get(&p, "name")?),
                        s(&get(&p, "where")?),
                        s(&get(&p, "version")?)
                    ));
                    Ok(0)
                })
                .print())
            } else {
                Ok(Shown::out(format!(
                    "removed placement {name} (the value at that place is yours to delete)\n"
                ))
                .print())
            }
        }
        _ => {
            let q = if name.is_empty() {
                String::new()
            } else {
                py::urlencode(&merge(&[&sj, &[kv("name", &name)]]))
            };
            let r = ctx.call(false, "placements", &q)?;
            if json {
                return Ok(print_body(&r.body));
            }
            Ok(render(&r.body, list).print())
        }
    }
}

fn list(v: &Value, l: &mut Lines) -> R<i32> {
    let rows = items(&or_list(get(v, "placements")?))?;
    if rows.is_empty() {
        l.p(format!(
            "no placements recorded ({CLI} vault placement add NAME --where <host>:<path>)"
        ));
    }
    for p in rows {
        let where_ = if py::is(&get(&p, "target")?, "systemd_cred") {
            format!(
                "systemd {} -> {}",
                s(&get(&p, "credName")?),
                s(&get(&p, "unit")?)
            )
        } else {
            let w = get(&p, "where")?;
            if t(&w) {
                s(&w)
            } else {
                String::new()
            }
        };
        let note = if t(&get(&p, "note")?) {
            py::cat("  # ", &idx(&p, "note")?)?
        } else {
            String::new()
        };
        l.p(format!(
            "{}  {} v{}/{} {} {where_}{note}",
            s(&get(&p, "id")?),
            left(&get(&p, "name")?, 24),
            s(&get(&p, "version")?),
            s(&get(&p, "latestVersion")?),
            left(&get(&p, "state")?, 8)
        ));
    }
    if t(&get(v, "needNewerK2")?) {
        l.p(format!(
            "{} more need a newer {} to read",
            py::d(&idx(v, "needNewerK2")?)?,
            k2_client::brand::PRODUCT_NAME
        ));
    }
    Ok(0)
}
