//! The vault verbs with parsers of their own: sync (`_vault_sync`,
//! `cli/src/vault.sh`) and placement and placements (`_vault_placement`,
//! `cli/src/job.sh`), fetch (`_vault_fetch`), systemd-cred
//! ([`super::systemd`]) and run ([`super::run`]). A fetch line the binary
//! can't be sure of is the shell's ([`fetch_native`]), and so is run where
//! there is no `exec` (not Unix).

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
        "sync" | "placements" | "systemd-cred" => true,
        "run" => cfg!(unix),
        "fetch" => fetch_native(rest),
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
        "fetch" => fetch(ctx, rest),
        "systemd-cred" => super::systemd::run(ctx, rest),
        "run" => super::run::run(ctx, rest),
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

/// `$PWD` as the shell has it.
pub(super) fn pwd() -> Option<String> {
    crate::daemon::logical_cwd(std::env::var_os("PWD"))
        .ok()?
        .to_str()
        .map(str::to_string)
}

/// The fetch flags that take the next word.
const FETCH_VALUES: &[&str] = &[
    "--secret",
    "-s",
    "--scope",
    "-X",
    "--method",
    "-H",
    "--header",
    "-d",
    "--data",
    "--json",
    "--max-bytes",
    "--timeout",
    "-o",
    "--output",
    "--dst",
    "--fmt",
];

/// The value flags the shell reads with `"${2:-}"; shift 2` and no check
/// first: as the last word, its `shift 2` fails.
const FETCH_UNCHECKED: &[&str] = &["--max-bytes", "--timeout", "--dst", "--fmt"];

/// The fetch lines the binary answers: not one whose last word is a
/// value flag the shell reads unchecked ([`FETCH_UNCHECKED`]: its `shift
/// 2` fails there; a checked one, such as `--json`, is its usage line),
/// and not one with a file path `cli_abs_path` would make that the binary
/// isn't sure of.
fn fetch_native(rest: &[&str]) -> bool {
    if rest.last().is_some_and(|w| FETCH_UNCHECKED.contains(w)) {
        return false;
    }
    let Some(pwd) = pwd() else {
        return false;
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        if FETCH_VALUES.contains(&w) {
            // A checked flag as the last word: its usage line.
            let Some(&v) = rest.get(i + 1) else {
                break;
            };
            let path = match w {
                "-o" | "--output" => Some(v),
                "-d" | "--data" | "--json" => v.strip_prefix('@'),
                _ => None,
            };
            if let Some(p) = path {
                if crate::families::workspace::register::abs_path(p, &pwd).is_none() {
                    return false;
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    true
}

/// `_vault_fetch`: K2 makes the HTTPS call and adds the secret where its
/// creator said; the value never comes back.
fn fetch(ctx: &mut Ctx, rest: &[&str]) -> Result<Exit, Exit> {
    let (mut url, mut method, mut max_bytes, mut timeout) =
        (String::new(), String::new(), String::new(), String::new());
    let (mut include, mut out_file, mut body_inline, mut body_file, mut ctype, mut json_out) = (
        false,
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        false,
    );
    let (mut dst, mut fmt, mut use_scope, mut use_ws) =
        (String::new(), String::new(), String::new(), String::new());
    let mut secrets: Vec<String> = Vec::new();
    let mut headers: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let v = rest.get(i + 1).copied().unwrap_or("");
        let need = |line: &str| -> Result<(), Exit> {
            if v.is_empty() {
                Err(usage(line.into()))
            } else {
                Ok(())
            }
        };
        match w {
            "--secret" | "-s" => {
                need("usage: --secret NAME")?;
                secrets.push(v.into());
            }
            "--scope" => {
                if v == "session" || v == "server" {
                    use_scope = v.into();
                } else if let Some(ws) = v.strip_prefix("ws:").filter(|x| !x.is_empty()) {
                    use_scope = "workspace".into();
                    use_ws = ws.into();
                } else {
                    return Err(usage("usage: --scope session|server|ws:<name> (this session's own secret of that name, or one shared with you from there)".into()));
                }
            }
            "-X" | "--method" => {
                need("usage: -X METHOD")?;
                method = v.into();
            }
            "-H" | "--header" => {
                need("usage: -H 'Name: value'")?;
                headers.push(v.into());
            }
            "-d" | "--data" | "--json" => {
                if w == "--json" {
                    need("usage: --json @file | --json 'text'")?;
                } else {
                    need("usage: -d 'text' | -d @file")?;
                }
                match v.strip_prefix('@') {
                    Some(f) => body_file = f.into(),
                    None => body_inline = v.into(),
                }
                if w == "--json" {
                    ctype = "application/json".into();
                }
            }
            "--max-bytes" => max_bytes = v.into(),
            "--timeout" => timeout = v.into(),
            "-o" | "--output" => {
                need("usage: -o file")?;
                out_file = v.into();
            }
            "--dst" => dst = v.into(),
            "--fmt" => fmt = v.into(),
            "--include-headers" | "-i" => {
                include = true;
                i += 1;
                continue;
            }
            "--json-out" => {
                json_out = true;
                i += 1;
                continue;
            }
            "--system" => {
                return Err(usage(format!(
                    "usage: only {CLI} vault list takes --system"
                )))
            }
            _ if w.starts_with('-') => {
                return Err(usage(format!(
                    "usage: unknown fetch flag '{w}' ({CLI} vault fetch --help)"
                )))
            }
            _ => {
                if !url.is_empty() {
                    return Err(usage("usage: one URL".into()));
                }
                url = w.into();
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    if url.is_empty() {
        return Err(usage(format!("usage: {CLI} vault fetch --secret NAME URL")));
    }
    if secrets.is_empty() {
        return Err(usage("usage: name the secret: --secret NAME".into()));
    }
    if !ctype.is_empty() {
        headers.push(format!("Content-Type: {ctype}"));
    }
    let pwd = pwd().unwrap_or_default();
    let abs = |p: &str| crate::families::workspace::register::abs_path(p, &pwd).unwrap_or_default();
    if !body_file.is_empty() {
        body_file = abs(&body_file);
    }
    if !out_file.is_empty() {
        out_file = abs(&out_file);
    }
    let t_ = |x: &str| py::text(x);
    let k = |x: &str| Text::from(x);
    let mut o: Vec<(Text, Value)> = vec![(k("url"), t_(&url))];
    let mut ss = Vec::new();
    for name in &secrets {
        let mut e = vec![(k("name"), t_(name))];
        if !dst.is_empty() {
            e.push((k("dst"), t_(&dst)));
        }
        if !fmt.is_empty() {
            e.push((k("fmt"), t_(&fmt)));
        }
        if !use_scope.is_empty() {
            e.push((k("scope"), t_(&use_scope)));
        }
        if !use_ws.is_empty() {
            e.push((k("workspace"), t_(&use_ws)));
        }
        ss.push(Value::Object(e));
    }
    o.push((k("secrets"), Value::Array(ss)));
    let mut hs = Vec::new();
    for h in &headers {
        let Some((name, value)) = h.split_once(':') else {
            return Err(usage("usage: -H takes Name: value".into()));
        };
        let strip = |x: &str| x.trim_matches(crate::families::py_space).to_string();
        hs.push(Value::Array(vec![t_(&strip(name)), t_(&strip(value))]));
    }
    o.push((k("headers"), Value::Array(hs)));
    if !method.is_empty() {
        o.push((k("method"), t_(&method)));
    }
    for (key, raw) in [("maxBytes", &max_bytes), ("timeoutSecs", &timeout)] {
        if raw.is_empty() {
            continue;
        }
        match super::verbs::py_int(raw) {
            Some(n) => o.push((k(key), Value::Int(n))),
            None => {
                let r = crate::families::Raise::at(
                    19,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        crate::out::py::repr(&t_(raw))
                    ),
                );
                out::stderr_line(r.text().trim_end_matches('\n'));
                return Err(Exit::Usage);
            }
        }
    }
    if include {
        o.push((k("includeHeaders"), Value::Bool(true)));
    }
    if !out_file.is_empty() {
        o.push((k("out"), t_(&out_file)));
    }
    if !body_file.is_empty() {
        o.push((k("bodyFile"), t_(&body_file)));
    }
    if !body_inline.is_empty() {
        o.push((k("bodyB64"), t_(&super::verbs::b64(body_inline.as_bytes()))));
    }
    let r = ctx.call(true, "fetch", &dumps(&Value::Object(o)))?;
    if json_out {
        return Ok(print_body(&r.body));
    }
    // `sys.stdout.buffer.write(...)`: the body's bytes, as they are.
    let mut raw: Option<Vec<u8>> = None;
    let shown = render(&r.body, |v, l| {
        if include {
            l.p(format!("HTTP {}", s(&get(v, "status")?)));
            for kv in items(&or_list(get(v, "headers")?))? {
                let parts = items(&kv)?;
                if parts.len() != 2 {
                    return Err(crate::families::Raise::at(
                        6,
                        format!(
                            "ValueError: {} values to unpack (expected 2)",
                            if parts.len() > 2 {
                                "too many"
                            } else {
                                "not enough"
                            }
                        ),
                    ));
                }
                l.p(format!("{}: {}", s(&parts[0]), s(&parts[1])));
            }
            l.p("");
        }
        let body = get(v, "body")?;
        if t(&get(v, "out")?) {
            l.e(format!(
                "wrote {} bytes to {} (status {})",
                s(&get(v, "bytes")?),
                s(&idx(v, "out")?),
                s(&get(v, "status")?)
            ));
        } else if body != Value::Null {
            match &body {
                Value::Str(_) => l.out.push_str(&s(&body)),
                other => {
                    return Err(crate::families::Raise::at(
                        13,
                        format!(
                            "TypeError: write() argument must be str, not {}",
                            crate::out::py::type_name(other)
                        ),
                    ))
                }
            }
        } else if t(&get(v, "bodyB64")?) {
            let b = s(&idx(v, "bodyB64")?);
            match super::verbs::b64_decode(&b) {
                Some(bytes) => raw = Some(bytes),
                None => {
                    return Err(crate::families::Raise::at(
                        15,
                        "binascii.Error: Incorrect padding",
                    ))
                }
            }
        }
        if t(&get(v, "truncated")?) {
            l.e("\n(truncated at the size limit; --max-bytes or -o)");
        }
        let st = get(v, "status")?;
        let n = if t(&st) {
            match &st {
                Value::Int(x) => x.parse::<i64>().unwrap_or(0),
                Value::Str(x) => x.as_str().and_then(|x| x.trim().parse().ok()).unwrap_or(0),
                _ => 0,
            }
        } else {
            0
        };
        if n >= 400 && !include {
            l.e(format!("(HTTP {n})"));
        }
        Ok(0)
    });
    let Some(bytes) = raw else {
        return Ok(shown.print());
    };
    let mut all = shown.stdout.into_bytes();
    all.extend(bytes);
    if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &all) {
        return Ok(out::after_stdout_error(&e));
    }
    let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
    Ok(shown.exit)
}
