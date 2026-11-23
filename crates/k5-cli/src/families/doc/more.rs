//! `cores`, `cad`, `av`, the history verbs (`versions`, `show`,
//! `restore`, `version`, `revert`, `diff --version`), `export` and
//! `watch` (`tail -f`).

use super::print::{eq, print_undo, py_f1, v2};
use super::py::{self, d, gt, gt_or, ix, left, local_ms, num, rows, sg, st, tr, Pr, R};
use super::{m, w, Ctx, Step};
use crate::exit::Exit;
use crate::families::{py_len, Raise};
use crate::out::json::{Text, Value};
use crate::out::py as opy;
use crate::out::pylib::{difflib, ops, path as pypath};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::sync::atomic::{AtomicBool, Ordering};

const CORES: &[&str] = &[
    "markdown", "text", "any-file", "board", "slides", "timeline", "fab", "blend", "cad",
];

fn is_str(x: Option<&Value>, s: &str) -> bool {
    matches!(x, Some(Value::Str(t)) if t.as_str() == Some(s))
}

fn int0() -> Value {
    Value::Int("0".into())
}

pub fn cores(ctx: &mut Ctx, words: &[String]) -> Step {
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = &words[i];
        i += 1;
        match a.as_str() {
            "-h" | "--help" => return Ok(ctx.page("doc/cores")),
            // `rest+=("$1" "${2:-}")`: the value, or an empty one.
            "--host" | "--token" => {
                rest.push(a.clone());
                rest.push(words.get(i).cloned().unwrap_or_default());
                i += 1;
            }
            "--json" | "--token-stdin" => rest.push(a.clone()),
            x if x.starts_with("--host=") => rest.push(a.clone()),
            x if x.starts_with('-') => {
                return Err(ctx.usage(format!(
                    "{CLI_NAME} doc cores doesn't take {x} ({CLI_NAME} doc cores --help)"
                )))
            }
            _ => rest.push(a.clone()),
        }
    }
    let args = ctx.parse(&rest)?;
    let list =
        "(<core>: markdown | text | any-file | board | slides | timeline | fab | blend | cad)";
    let (action, core) = match args.len() {
        0 => {
            let b = ctx.call(false, "/cli/docs/cores", "")?;
            return Ok(ctx.json_or(&b, cores_list));
        }
        2 => (args[0].clone(), args[1].clone()),
        _ => return Err(ctx.usage(format!("{CLI_NAME} doc cores [on|off <core>]  {list}"))),
    };
    if action != "on" && action != "off" {
        return Err(ctx.usage(format!("{CLI_NAME} doc cores on|off <core>  {list}")));
    }
    if !CORES.contains(&core.as_str()) {
        return Err(ctx.usage(format!(
            "no such core: {core} (markdown, text, any-file, board, slides, timeline, fab, blend, cad)"
        )));
    }
    if ctx.passport {
        let mut err = String::new();
        if !ctx.json {
            err.push_str(&format!("Error [person_only]: {CLI_NAME} doc cores on|off is for people, from a terminal outside {PRODUCT_NAME} or Settings → Cores → Multiplayer. An agent can list the cores ({CLI_NAME} doc cores) and asks its human ({CLI_NAME} thread) to turn one on.\n"));
        }
        err.push_str(&py::dumps(&py::obj(vec![(
            "error",
            py::obj(vec![
                ("code", py::s("person_only")),
                (
                    "hint",
                    py::s(&format!("{CLI_NAME} doc cores on|off is for people")),
                ),
            ]),
        )])));
        err.push('\n');
        return Err(crate::families::Shown::err(err, Exit::Refused).print());
    }
    let body = py::dumps(&py::obj(vec![(
        "multiplayerCores",
        Value::Object(vec![(
            Text::from(core.as_str()),
            Value::Bool(action == "on"),
        )]),
    )]));
    let (status, out) = ctx.send(true, "/cli/settings/update", &body)?;
    let json = ctx.json;
    let mut pr = Pr::default();
    if status != "200" {
        let r = cores_fail(&mut pr, &status, &out, json);
        return Err(pr.finish(r));
    }
    let r = py::load(&out, 4).and_then(|v| cores_done(&mut pr, &v, &core, &action, json));
    Ok(pr.finish(r))
}

fn cores_fail(pr: &mut Pr, status: &str, out: &[u8], json: bool) -> R<Exit> {
    let raw = py::stdin_text(out, 4)?.into_owned();
    let v = crate::out::pylib::json::loads(&raw).ok();
    let mut hint = py::s("");
    if let Some(Value::Object(pairs)) = &v {
        match crate::families::get(pairs, "error") {
            Some(e @ Value::Str(_)) => hint = e.clone(),
            Some(Value::Object(ep)) => {
                hint = crate::families::get(ep, "hint")
                    .filter(|h| opy::truthy(h))
                    .or_else(|| crate::families::get(ep, "message").filter(|m| opy::truthy(m)))
                    .cloned()
                    .unwrap_or(py::s(""));
            }
            _ => {}
        }
    }
    if !opy::truthy(&hint) {
        let stripped: String = raw
            .trim_matches(crate::families::py_space)
            .chars()
            .take(300)
            .collect();
        hint = py::s(&if !stripped.is_empty() {
            stripped
        } else if status == "000" {
            format!("no answer from the {PRODUCT_NAME} daemon")
        } else {
            format!("the daemon refused (HTTP {status})")
        });
    }
    let code = match status {
        "400" => "bad_request",
        "401" | "403" => "refused",
        "000" => "daemon_unreachable",
        _ => "error",
    };
    let rc = match status {
        "400" => Exit::Usage,
        "401" | "403" => Exit::Refused,
        _ => Exit::Error,
    };
    if !json {
        pr.eline(&format!("Error: {}", st(&hint)));
    }
    let n = status.trim_start_matches('0');
    let st_v = if status.bytes().all(|b| b.is_ascii_digit()) && !status.is_empty() {
        Value::Int(if n.is_empty() { "0".into() } else { n.into() })
    } else {
        int0()
    };
    pr.eline(&py::dumps(&py::obj(vec![(
        "error",
        py::obj(vec![
            ("code", py::s(code)),
            ("hint", hint),
            ("status", st_v),
        ]),
    )])));
    Ok(rc)
}

fn cores_done(pr: &mut Pr, v: &Value, core: &str, action: &str, json: bool) -> R<Exit> {
    let r = gt_or(v, "multiplayerCoresReport", 5)?
        .cloned()
        .unwrap_or_else(|| Value::Object(Vec::new()));
    let failed = gt_or(&r, "failed", 6)?
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let rc = if opy::truthy(&failed) {
        Exit::Error
    } else {
        Exit::Ok
    };
    if json {
        let mc = gt_or(v, "multiplayerCores", 8)?
            .cloned()
            .unwrap_or_else(|| Value::Object(Vec::new()));
        pr.line(&py::dumps(&py::obj(vec![
            ("multiplayerCores", mc),
            ("multiplayerCoresReport", r),
        ])));
        return Ok(rc);
    }
    let name = match core {
        "markdown" => "The Markdown core".to_string(),
        "text" => "The text and code core".to_string(),
        "any-file" => "The any-file core".to_string(),
        "board" => "The Board core".to_string(),
        c => format!("The {c} core"),
    };
    let mut changed = false;
    for c in rows(&r, "changed", 15)? {
        if is_str(gt(&c, "core", 15)?, core) {
            changed = true;
            break;
        }
    }
    if !changed {
        pr.line(&format!("{name} is already {action}."));
    } else if core == "board" {
        pr.line(&if action == "on" {
            format!("The Board core is on. Board Gardens show their boards and {CLI_NAME} board works.")
        } else {
            format!("The Board core is off. Board Gardens show a placeholder and {CLI_NAME} board answers core_off; the boards are kept.")
        });
    } else if action == "on" {
        pr.line(&format!(
            "{name} is on. Its files are live docs on this server from now on."
        ));
    } else {
        let fl = py::iter(&failed, 24)?;
        let mut bad = Vec::new();
        for f in &fl {
            bad.push(gt(f, "path", 24)?.cloned().unwrap_or(Value::Null));
        }
        let closed = gt_or(&r, "closed", 25)?.cloned().unwrap_or_else(int0);
        let closed = py::int(&closed, 25)?;
        let nf = py_len(&failed, 25)? as i128;
        let n = match &closed {
            Value::Int(t) => t.parse::<i128>().unwrap_or(0) - nf,
            _ => -nf,
        };
        let mut paths = Vec::new();
        for p in rows(&r, "paths", 26)? {
            if !bad.iter().any(|b| ops::py_eq(b, &p)) {
                paths.push(p);
            }
        }
        let mut line = if n <= 0 && !opy::truthy(&failed) {
            format!("{name} is off. No files were open live.")
        } else if n <= 0 {
            format!("{name} is off.")
        } else {
            let shown = if paths.is_empty() {
                String::new()
            } else {
                format!(" ({})", ops::join(&Value::Array(paths), ", ", 33)?)
            };
            format!(
                "{name} is off. {n} live file{} written to disk and closed{shown}.",
                if n == 1 { " was" } else { "s were" }
            )
        };
        if opy::truthy(&failed) {
            let mut each = Vec::new();
            for f in &fl {
                each.push(format!("{} ({})", sg(f, "path", 37)?, sg(f, "error", 37)?));
            }
            let one = fl.len() == 1;
            line += &format!(
                " {} file{} couldn't be written: {}; {} changes are kept in {PRODUCT_NAME}'s history and are written when the core is turned on again.",
                fl.len(),
                if one { "" } else { "s" },
                each.join(", "),
                if one { "its" } else { "their" }
            );
        }
        pr.line(&line);
    }
    Ok(rc)
}

fn cores_list(pr: &mut Pr, v: &Value) -> R<Exit> {
    pr.line("Multiplayer cores on this server (the owner or an Admin turns them on or off):");
    for c in rows(v, "cores", 5)? {
        let pill = if is_str(gt(&c, "status", 6)?, "beta") {
            " (Beta)"
        } else {
            ""
        };
        let id = ix(&c, "id", 7)?.clone();
        let on = if tr(&c, "on", 7)? { "on" } else { "off" };
        let name = st(ix(&c, "name", 7)?);
        let short = match gt_or(&c, "short", 7)? {
            Some(x) => st(x),
            None => gt_or(&c, "description", 7)?.map_or_else(String::new, st),
        };
        pr.line(&format!(
            "  {} {}  {name}{pill}: {short}",
            left(Some(&id), 10),
            crate::families::ljust(on, 4)
        ));
        let o = gt_or(&c, "open", 8)?
            .cloned()
            .unwrap_or_else(|| Value::Object(Vec::new()));
        if tr(&c, "on", 9)? && tr(&o, "files", 9)? {
            let n = ix(&o, "files", 10)?.clone();
            let shown = match gt_or(&o, "paths", 11)? {
                Some(p) => ops::join(p, ", ", 11)?,
                None => String::new(),
            };
            let one = ops::py_eq(&n, &Value::Int("1".into()));
            pr.line(&format!(
                "  {}       open live now: {} file{}{}",
                crate::families::ljust("", 10),
                d(&n, 12)?,
                if one { "" } else { "s" },
                if shown.is_empty() {
                    String::new()
                } else {
                    format!(" ({shown})")
                }
            ));
        }
    }
    pr.line(&format!(
        "While a core is off, {CLI_NAME} doc refuses its files (core_off, exit 3) and people open them the usual way."
    ));
    Ok(Exit::Ok)
}

/// `_doc_cad`: a drawing's read.
pub fn cad(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut k2args, mut toks) = (Vec::new(), Vec::<String>::new());
    let mut last: Option<usize> = None;
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "-w" | "--workspace" | "--on-behalf-of" | "--host" | "--token" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(ctx.usage(format!("{a} needs a value")));
                };
                k2args.push(a.to_string());
                k2args.push(v.clone());
                i += 2;
                continue;
            }
            "--json" => k2args.push(a.to_string()),
            "--" => {
                for t in &words[i + 1..] {
                    toks.push(t.clone());
                    last = Some(toks.len() - 1);
                }
                break;
            }
            "--token-stdin" => k2args.push(a.to_string()),
            _ if a.starts_with("--workspace=")
                || a.starts_with("--on-behalf-of=")
                || a.starts_with("--host=") =>
            {
                k2args.push(a.to_string())
            }
            _ if a.starts_with('-') => toks.push(a.to_string()),
            _ => {
                toks.push(a.to_string());
                last = Some(toks.len() - 1);
            }
        }
        i += 1;
    }
    let Some(last) = last.filter(|l| *l >= 1) else {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc cad <read verb> [args] <drawing>  ({CLI_NAME} doc cad --help)"
        )));
    };
    let path = toks.remove(last);
    ctx.parse(&k2args)?;
    let path = ctx.path(&path);
    let argv = py::dumps(&Value::Array(toks.iter().map(|t| py::s(t)).collect()));
    let q = ctx.query(&path, &[("argv", &argv)]);
    let b = ctx.call(false, "/cli/docs/cad/read", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        if let Some(o) = gt_or(v, "stdout", 4)? {
            let o = py::text(o, "endswith", 6)?;
            pr.put(&o);
            if !o.ends_with('\n') {
                pr.put("\n");
            }
        }
        let mut nxt = match gt_or(v, "next", 7)? {
            Some(n) => py::iter(n, 7)?,
            None => Vec::new(),
        };
        while let Some(i) = nxt.iter().position(|x| is_str(Some(x), "--version")) {
            let end = (i + 2).min(nxt.len());
            nxt.drain(i..end);
        }
        if !nxt.is_empty() {
            let joined = ops::join(&Value::Array(nxt), " ", 12)?;
            let p = gt(v, "path", 12)?.map_or_else(String::new, st);
            pr.eline(&format!("more: {CLI_NAME} doc cad {joined} {p}"));
        }
        Ok(Exit::Ok)
    }))
}

/// The drawings under a folder, as `find DIR -type f \( -iname '*.dxf'
/// -o -iname '*.dwg' \) | LC_ALL=C sort` lists them.
fn drawings(dir: &str) -> Vec<String> {
    fn walk(at: &str, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(at) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = if at.ends_with('/') {
                format!("{at}{name}")
            } else {
                format!("{at}/{name}")
            };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(&p, out);
            } else if ft.is_file() {
                let l = name.to_ascii_lowercase();
                if l.ends_with(".dxf") || l.ends_with(".dwg") {
                    out.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    out
}

/// `_doc_av`: the AV reading of a set of drawings.
pub fn av(ctx: &mut Ctx, words: &[String]) -> Step {
    let what = words.first().map_or("", String::as_str).to_string();
    if what != "read" && what != "summary" {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc av read|summary --config <file> <drawing|folder>...  ({CLI_NAME} doc av --help)"
        )));
    }
    let words = &words[1..];
    let mut config = String::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        if a == "--config" {
            let Some(v) = words.get(i + 1) else {
                return Err(ctx.usage("--config needs a file"));
            };
            config = v.clone();
            i += 2;
            continue;
        }
        match eq(a, "--config") {
            Some(v) => config = v.to_string(),
            None => rest.push(a.to_string()),
        }
        i += 1;
    }
    if config.is_empty() {
        return Err(ctx.usage(format!("{CLI_NAME} doc av {what} needs --config <file>")));
    }
    let args = ctx.parse(&rest)?;
    if args.is_empty() {
        return Err(ctx.usage(format!("name the drawings (or a folder of them): {CLI_NAME} doc av {what} --config <file> <drawing|folder>...")));
    }
    let mut paths = Vec::new();
    for a in &args {
        if std::path::Path::new(a).is_dir() {
            for f in drawings(a) {
                paths.push(ctx.path(&f));
            }
        } else {
            paths.push(ctx.path(a));
        }
    }
    if paths.is_empty() {
        return Err(ctx.usage("no .dxf or .dwg drawing there"));
    }
    let config = ctx.path(&config);
    let pj = py::dumps(&Value::Array(paths.iter().map(|p| py::s(p)).collect()));
    let q = ctx.query("", &[("config", &config), ("paths", &pj), ("what", &what)]);
    let b = ctx.call(false, "/cli/docs/av/read", &q)?;
    Ok(ctx.json_or(&b, |pr, v| {
        let k = if is_str(gt(v, "what", 4)?, "summary") {
            "summary"
        } else {
            "stdout"
        };
        if let Some(o) = gt_or(v, k, 4)? {
            let o = py::text(o, "endswith", 6)?;
            pr.put(&o);
            if !o.ends_with('\n') {
                pr.put("\n");
            }
        }
        Ok(Exit::Ok)
    }))
}

fn size(n: Option<&Value>, line: u32) -> R<String> {
    let n = n.filter(|x| opy::truthy(x)).cloned().unwrap_or_else(int0);
    let x = num(&n, "<", "int", line).map_err(|_| {
        Raise::at(
            line,
            format!(
                "TypeError: '<' not supported between instances of '{}' and 'int'",
                opy::type_name(&n)
            ),
        )
    })?;
    Ok(if x < 1024.0 {
        format!("{} B", d(&n, line)?)
    } else if x < 1_048_576.0 {
        format!("{} KiB", py_f1(x / 1024.0))
    } else {
        format!("{} MiB", py_f1(x / 1_048_576.0))
    })
}

/// `"%10s" % s`.
fn right(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len >= n {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(n - len))
    }
}

/// `_doc_history VERB`: versions, show, restore, version and revert.
pub fn history(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let (mut ver, mut name, mut partial, mut outf) =
        (String::new(), String::new(), "@false", String::new());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--version" => {
                ver = v2(words, i)?;
                i += 2;
                continue;
            }
            "--name" => {
                name = v2(words, i)?;
                i += 2;
                continue;
            }
            "--partial" => partial = "@true",
            "--out" | "-o" => {
                if verb != "show" {
                    return Err(ctx.usage(format!("--out is for {CLI_NAME} doc show")));
                }
                outf = words.get(i + 1).cloned().unwrap_or_default();
                if outf.is_empty() {
                    return Err(ctx.usage("--out needs a file"));
                }
                i += 2;
                continue;
            }
            _ => {
                if let Some(v) = eq(a, "--version") {
                    ver = v.into();
                } else if let Some(v) = eq(a, "--name") {
                    name = v.into();
                } else if let Some(v) = eq(a, "--out") {
                    if verb != "show" {
                        return Err(ctx.usage(format!("--out is for {CLI_NAME} doc show")));
                    }
                    outf = v.into();
                } else {
                    rest.push(a.to_string());
                }
            }
        }
        i += 1;
    }
    if !ver.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ctx.usage(format!(
            "--version takes a version id ({CLI_NAME} doc versions)"
        )));
    }
    let args = ctx.parse(&rest)?;
    let mut change = String::new();
    let path = if verb == "revert" {
        if args.len() != 2 {
            return Err(ctx.usage(format!(
                "{CLI_NAME} doc revert <change> [--partial] <file.md>"
            )));
        }
        change = args[0].clone();
        if change.is_empty() || !change.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ctx.usage(format!("the change is a number ({CLI_NAME} doc log)")));
        }
        ctx.path(&args[1])
    } else {
        ctx.one_path(verb, &args)?
    };
    match verb {
        "versions" => {
            let q = ctx.query(&path, &[]);
            let b = ctx.call(false, "/cli/docs/versions", &q)?;
            Ok(ctx.json_or(&b, versions_lines))
        }
        "show" => {
            if ver.is_empty() {
                return Err(ctx.usage(format!(
                    "{CLI_NAME} doc show --version V [--out FILE] <file>"
                )));
            }
            let q = ctx.query(&path, &[("version", &ver)]);
            ctx.tmp()?;
            let ans = ctx.xfer(false, "/cli/docs/show", &q)?;
            if ctx.json {
                let mut b = ans.clone();
                b.push(b'\n');
                return Ok(
                    match crate::out::write_flushed(&mut std::io::stdout().lock(), &b) {
                        Ok(()) => Exit::Ok,
                        Err(e) => crate::out::after_stdout_error(&e),
                    },
                );
            }
            let mut pr = Pr::default();
            let r = show_lines(&mut pr, &ans, &outf, crate::families::sim::is_tty(1));
            Ok(pr.finish(r))
        }
        "restore" => {
            if ver.is_empty() {
                return Err(ctx.usage(format!("{CLI_NAME} doc restore --version V <file>")));
            }
            let body = ctx.body_arg(&path, &[("version", m(format!("@int:{ver}")))]);
            let b = ctx.call(true, "/cli/docs/restore", &body)?;
            Ok(ctx.json_or(&b, |pr, v| {
                let r = gt_or(v, "result", 4)?
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Vec::new()));
                if gt(v, "tier", 5)?.is_some_and(|t| ops::py_eq(t, &int0())) {
                    if tr(&r, "changed", 6)? {
                        let later = if tr(&r, "written", 8)? {
                            ""
                        } else {
                            "; the file is written back shortly"
                        };
                        pr.eline(&format!(
                            "restored version {} as version {} ({} bytes){later}",
                            sg(v, "restored", 7)?,
                            sg(&r, "version", 7)?,
                            sg(&r, "size", 7)?
                        ));
                    } else {
                        pr.eline(&format!(
                            "version {} already holds those bytes; nothing new recorded",
                            sg(&r, "version", 10)?
                        ));
                    }
                } else {
                    pr.eline(&format!(
                        "restored version {} as a new change (+{} -{}) · v{}; {CLI_NAME} doc undo reverses it",
                        sg(v, "restored", 12)?,
                        gt(&r, "linesAdded", 12)?.map_or_else(|| "0".into(), st),
                        gt(&r, "linesRemoved", 12)?.map_or_else(|| "0".into(), st),
                        sg(&r, "version", 12)?
                    ));
                }
                Ok(Exit::Ok)
            }))
        }
        "version" => {
            if name.is_empty() {
                return Err(ctx.usage(format!(
                    "{CLI_NAME} doc version --name \"<name>\" [--version V] <file.md>"
                )));
            }
            let mut kv = vec![("name", w(name))];
            if !ver.is_empty() {
                kv.push(("version", m(format!("@int:{ver}"))));
            }
            let body = ctx.body_arg(&path, &kv);
            let b = ctx.call(true, "/cli/docs/name", &body)?;
            Ok(ctx.json_or(&b, |pr, v| {
                pr.line(&format!(
                    "version {} named \"{}\"",
                    sg(v, "version", 1)?,
                    sg(v, "name", 1)?
                ));
                Ok(Exit::Ok)
            }))
        }
        _ => {
            let body = ctx.body_arg(
                &path,
                &[
                    ("change", m(format!("@int:{change}"))),
                    ("partial", m(partial)),
                ],
            );
            let b = ctx.call(true, "/cli/docs/revert", &body)?;
            Ok(print_undo(ctx, "revert", &b))
        }
    }
}

fn versions_lines(pr: &mut Pr, v: &Value) -> R<Exit> {
    let tier0 = gt(v, "tier", 9)?.is_some_and(|t| ops::py_eq(t, &int0()));
    for x in rows(v, "versions", 7)? {
        let at_ms = gt_or(&x, "atMs", 8)?.cloned().unwrap_or_else(int0);
        let t = local_ms(&at_ms, "%Y-%m-%d %H:%M:%S", 8)?;
        let name = match gt_or(&x, "name", 9)? {
            Some(_) => format!(" \"{}\"", st(ix(&x, "name", 9)?)),
            None => String::new(),
        };
        let tier0 = gt(v, "tier", 10)?.is_some_and(|t| ops::py_eq(t, &int0())) && tier0;
        if tier0 {
            let mut extra = String::new();
            if gt(&x, "restoredFrom", 12)?.is_some_and(|r| *r != Value::Null) {
                extra += &format!(" (from version {})", st(ix(&x, "restoredFrom", 13)?));
            }
            if tr(&x, "conflictCopy", 14)? {
                extra += &format!(
                    " (kept the replaced one as \"{}\")",
                    st(ix(&x, "conflictCopy", 15)?)
                );
            }
            if tr(&x, "redacted", 16)? {
                extra += " [purged]";
            }
            let who = gt_or(&x, "authorLabel", 19)?.or(gt(&x, "author", 19)?);
            let cur = if tr(&x, "current", 19)? {
                "  (current)"
            } else {
                ""
            };
            pr.line(&format!(
                "{} {t}  {} {}  {} via {}{name}{extra}{cur}",
                left(gt(&x, "id", 18)?, 6),
                left(gt(&x, "reason", 18)?, 8),
                right(&size(gt(&x, "size", 18)?, 18)?, 10),
                py::so(who),
                sg(&x, "via", 19)?
            ));
        } else {
            let authors = match gt_or(&x, "authors", 21)? {
                Some(a) => ops::join(a, ", ", 21)?,
                None => String::new(),
            };
            pr.line(&format!(
                "{} {t}  at change {} {}{name}  {authors}",
                left(gt(&x, "id", 21)?, 6),
                left(gt(&x, "version", 21)?, 6),
                left(gt(&x, "reason", 21)?, 16)
            ));
        }
    }
    Ok(Exit::Ok)
}

fn show_lines(pr: &mut Pr, ans: &[u8], outf: &str, tty: bool) -> R<Exit> {
    let text = match std::str::from_utf8(ans) {
        Ok(t) => t.replace("\r\n", "\n").replace('\r', "\n"),
        Err(e) => return Err(super::utf8_raise(3, ans, e)),
    };
    let v = crate::out::pylib::json::loads(&text).map_err(|f| Raise::at(3, f.last_line()))?;
    let has_b64 = ops::contains(&v, "bytesB64", 5)?;
    let data: Vec<u8> = if has_b64 {
        match ix(&v, "bytesB64", 5)? {
            b @ Value::Str(_) => crate::families::compute::writes::fetch::b64decode(&st(b))
                .map_err(|e| Raise::at(5, e))?,
            other => return Err(py::b64_type(other, 5)),
        }
    } else {
        match gt_or(&v, "text", 5)? {
            Some(t) => py::text(t, "encode", 5)?.into_bytes(),
            None => Vec::new(),
        }
    };
    if !outf.is_empty() {
        std::fs::write(outf, &data)
            .map_err(|e| Raise::at(7, crate::out::pylib::json::os_error(&e, outf).last_line()))?;
        pr.eline(&format!(
            "wrote version {} of {} ({} bytes) to {outf}",
            sg(&v, "version", 9)?,
            st(ix(ix(&v, "doc", 9)?, "path", 9)?),
            data.len()
        ));
        return Ok(Exit::Ok);
    }
    if tty && (data.contains(&0) || std::str::from_utf8(&data).is_err()) {
        let hint = format!(
            "version {} of {} is binary ({} bytes); write it to a file with --out FILE, or pipe it",
            sg(&v, "version", 18)?,
            st(ix(ix(&v, "doc", 18)?, "path", 18)?),
            data.len()
        );
        pr.eline(&format!("Error: {hint}"));
        pr.eline(&py::dumps(&py::obj(vec![(
            "error",
            py::obj(vec![
                ("code", py::s("binary_output")),
                ("hint", py::s(&hint)),
            ]),
        )])));
        return Ok(Exit::Usage);
    }
    pr.out.extend_from_slice(&data);
    Ok(Exit::Ok)
}

/// `diff --version V [--to W] [-U N]`: a diff between versions.
pub fn history_diff(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut from, mut to, mut cx) = (String::new(), String::new(), String::new());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        let slot = match a {
            "--version" => &mut from,
            "--to" => &mut to,
            "-U" => &mut cx,
            "-u" => {
                i += 1;
                continue;
            }
            _ => {
                match eq(a, "--version") {
                    Some(v) => from = v.into(),
                    None => rest.push(a.to_string()),
                }
                i += 1;
                continue;
            }
        };
        *slot = v2(words, i)?;
        i += 2;
    }
    if !format!("{from}{to}{cx}")
        .bytes()
        .all(|b| b.is_ascii_digit())
    {
        return Err(ctx.usage("--version, --to and -U take numbers"));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("diff", &args)?;
    let q = ctx.query(
        &path,
        &[("fromVersion", &from), ("toVersion", &to), ("context", &cx)],
    );
    let b = ctx.call(false, "/cli/docs/diff", &q)?;
    // Two commands: the text (`printf` or a Python), then a second Python
    // whose exit is the command's, whatever the first did (its traceback
    // included).
    let mut pr = Pr::default();
    if ctx.json {
        pr.out.extend_from_slice(&b);
        pr.out.push(b'\n');
    } else {
        let r = py::load(&b, 1).and_then(|v| {
            let t = gt(&v, "text", 1)?.cloned().unwrap_or(py::s(""));
            pr.put(&py::write_arg(&t, 1)?);
            Ok(Exit::Ok)
        });
        if r.is_err() {
            let _ = pr.finish(r);
            pr = Pr::default();
        }
    }
    let mut text = b.clone();
    text.push(b'\n');
    let r = py::load(&text, 1).and_then(|v| py::exit_int(gt(&v, "exit", 1)?.unwrap_or(&int0()), 1));
    Ok(pr.finish(r))
}

static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// The shell's `trap ... INT TERM` for a loop: Ctrl-C or SIGTERM sets a
/// flag the loop reads.
fn catch_stop() {
    #[cfg(unix)]
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

fn release_stop() {
    #[cfg(unix)]
    // SAFETY: back to the default action.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }
}

/// `sleep SECS`, cut short by the stop flag. True when stopped.
fn nap(secs: f64) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
    while std::time::Instant::now() < end {
        if STOP.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    STOP.load(Ordering::SeqCst)
}

/// `_doc_export`.
pub fn export(ctx: &mut Ctx, words: &[String]) -> Step {
    let (mut to, mut ver, mut outf, mut list, mut node) = (
        String::new(),
        String::new(),
        String::new(),
        false,
        String::new(),
    );
    let mut opts: Vec<String> = Vec::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--list" => list = true,
            "--node" => {
                node = ctx.need(
                    words,
                    i,
                    &format!("--node needs a layer address ({CLI_NAME} doc outline <file>)"),
                )?;
                i += 1;
            }
            "--to" => {
                to = ctx.need(
                    words,
                    i,
                    &format!(
                        "--to needs a target ({CLI_NAME} doc export --list <file> shows them)"
                    ),
                )?;
                i += 1;
            }
            "--version" => {
                ver = ctx.need(
                    words,
                    i,
                    &format!("--version needs a version id ({CLI_NAME} doc versions <file>)"),
                )?;
                i += 1;
            }
            "--option" | "-O" => {
                opts.push(ctx.need(words, i, "--option needs NAME=VALUE")?);
                i += 1;
            }
            "--out" | "-o" => {
                outf = ctx.need(words, i, "--out needs a file")?;
                i += 1;
            }
            _ => {
                if let Some(v) = eq(a, "--node") {
                    node = v.into();
                } else if let Some(v) = eq(a, "--to") {
                    to = v.into();
                } else if let Some(v) = eq(a, "--version") {
                    ver = v.into();
                } else if let Some(v) = eq(a, "--option") {
                    opts.push(v.into());
                } else if let Some(v) = eq(a, "--out") {
                    outf = v.into();
                } else {
                    rest.push(a.to_string());
                }
            }
        }
        i += 1;
    }
    if !ver.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ctx.usage(format!(
            "--version takes a version id (the first column of {CLI_NAME} doc versions <file>)"
        )));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("export", &args)?;
    if list {
        if !(to.is_empty() && ver.is_empty() && outf.is_empty() && opts.is_empty()) {
            return Err(ctx.usage(format!(
                "{CLI_NAME} doc export --list <file> takes no other export flags"
            )));
        }
        let q = ctx.query(&path, &[]);
        let b = ctx.call(false, "/cli/docs/export/targets", &q)?;
        return Ok(ctx.json_or(&b, targets_lines));
    }
    if to.is_empty() {
        return Err(ctx.usage(format!("{CLI_NAME} doc export --to <target> [--version V] [--option NAME=VALUE]... [--out FILE] <file>  ({CLI_NAME} doc export --list <file> shows the targets)")));
    }
    // --out: from where you stand (absolute), unless -w names the
    // workspace it is relative to. On --host it is a path on this
    // machine: the binary writes it (fetch).
    let remote = ctx.client.as_ref().is_some_and(|c| c.force_tcp);
    let (mut local_out, mut send_out) = (String::new(), String::new());
    if !outf.is_empty() {
        if remote {
            local_out = pypath::abspath(&outf);
        } else if !ctx.ws.is_empty() && !outf.starts_with('/') {
            send_out = outf.clone();
        } else {
            local_out = pypath::abspath(&outf);
            send_out = local_out.clone();
        }
        if !local_out.is_empty() && std::fs::symlink_metadata(&local_out).is_ok() {
            return Err(ctx.die(
                "exists",
                format!(
                    "{outf} already exists; an export never overwrites a file (name another one)"
                ),
                Exit::Usage,
            ));
        }
    }
    let mut dd: Vec<(&str, Value)> = vec![
        ("path", py::s(&path)),
        ("target", py::s(&to)),
        (
            "options",
            Value::Array(opts.iter().map(|o| py::s(o)).collect()),
        ),
        ("waitMs", Value::Int("2000".into())),
    ];
    if !ctx.ws.is_empty() {
        dd.push(("workspace", py::s(&ctx.ws)));
    }
    if !ctx.ob.is_empty() {
        dd.push(("onBehalfOf", py::s(&ctx.ob)));
    }
    if !ver.is_empty() {
        dd.push((
            "version",
            Value::Int(py::int_text(&ver).unwrap_or_default()),
        ));
    }
    if !send_out.is_empty() {
        dd.push(("out", py::s(&send_out)));
    }
    if remote && !outf.is_empty() {
        dd.push(("fetch", Value::Bool(true)));
    }
    if !node.is_empty() {
        dd.push(("node", py::s(&node)));
    }
    ctx.tmp()?;
    let body = py::dumps(&py::obj(dd)) + "\n";
    let ans = ctx.xfer(true, "/cli/docs/export", &body)?;
    let mut cur = super::substituted(&ans);
    let field = |b: &[u8], k: &str| -> Result<String, Exit> {
        let r = py::load(b, 1).and_then(|v| Ok(gt(&v, k, 1)?.map_or_else(String::new, st)));
        r.map_err(|e| Pr::default().finish(Err(e)))
    };
    let job = field(&cur, "job")?;
    let mut state = field(&cur, "state")?;
    if state != "done" {
        let tty = crate::families::sim::is_tty(2);
        STOP.store(false, Ordering::SeqCst);
        catch_stop();
        let mut cancelled = false;
        while state != "done" {
            if STOP.load(Ordering::SeqCst) && !cancelled {
                let body = ctx.body_arg("", &[("job", w(job.clone()))]);
                ctx.call(true, "/cli/docs/export/cancel", &body)?;
                cancelled = true;
            }
            nap(0.5);
            let q = ctx.query("", &[("job", &job)]);
            cur = ctx.call(false, "/cli/docs/export/status", &q)?;
            state = field(&cur, "state")?;
            if tty && state != "done" {
                let mut pr = Pr::default();
                let r = py::load(&cur, 3).and_then(|v| {
                    let dn = gt_or(&v, "done", 4)?.cloned().unwrap_or_else(int0);
                    let t = gt_or(&v, "total", 4)?.cloned().unwrap_or_else(int0);
                    if opy::truthy(&t) {
                        let p =
                            (100.0 * num(&dn, "*", "int", 5)? / num(&t, "//", "int", 5)?).floor();
                        pr.eput(&format!("\rexporting… {}%  ", p as i64));
                    } else {
                        pr.eput("\rexporting…  ");
                    }
                    Ok(Exit::Ok)
                });
                if r.is_err() {
                    release_stop();
                    return Err(pr.finish(r));
                }
                pr.finish(r);
            }
        }
        release_stop();
        if tty {
            let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), b"\r\x1b[K");
        }
    }
    let fetch = match py::load(&cur, 1).and_then(|v| tr(&v, "fetch", 1)) {
        Ok(f) => f,
        Err(e) => return Err(Pr::default().finish(Err(e))),
    };
    if fetch {
        fetch_to(ctx, &job, &outf, &local_out)?;
    }
    if ctx.json {
        let mut pr = Pr::default();
        let r = py::load(&cur, 3).map(|mut v| {
            if !local_out.is_empty() {
                if let Value::Object(pairs) = &mut v {
                    match pairs.iter_mut().find(|(k, _)| k.as_str() == Some("wrote")) {
                        Some(slot) => slot.1 = py::s(&local_out),
                        None => pairs.push((Text::from("wrote"), py::s(&local_out))),
                    }
                }
            }
            pr.line(&py::dumps(&v));
            Exit::Ok
        });
        return Ok(pr.finish(r));
    }
    let mut pr = Pr::default();
    let r = py::load(&cur, 3).and_then(|v| export_lines(&mut pr, &v, &local_out, remote));
    Ok(pr.finish(r))
}

/// The fetch: the bytes in chunks to a temp name beside `--out`, linked
/// into place only when whole.
fn fetch_to(ctx: &mut Ctx, job: &str, outf: &str, local_out: &str) -> Result<(), Exit> {
    if local_out.is_empty() {
        return Err(ctx.die(
            "fetch_without_out",
            "the daemon kept the export for you to fetch, but no --out names where (pass --out FILE)",
            Exit::Usage,
        ));
    }
    let (dir, base) = pypath::split(local_out);
    let dir = if dir.is_empty() { ".".to_string() } else { dir };
    let part = format!("{dir}/.{base}.k2x-{job}.part");
    if std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .is_err()
    {
        return Err(ctx.die(
            "io",
            format!("can't write {part} (no such folder, or no permission)"),
            Exit::Error,
        ));
    }
    let mut offset = "0".to_string();
    loop {
        let q = ctx.query("", &[("job", job), ("offset", &offset)]);
        let (status, chunk) = ctx.xfer_raw(false, "/cli/docs/export/fetch", &q)?;
        if status != "200" {
            let _ = std::fs::remove_file(&part);
            return Err(super::fail(&status, &super::substituted(&chunk), ctx.json));
        }
        let r = append_chunk(&chunk, &part);
        match r {
            Ok((next, eof)) => {
                offset = next;
                if eof {
                    break;
                }
            }
            Err(e) => {
                Pr::default().finish(Err(e));
                let _ = std::fs::remove_file(&part);
                return Err(ctx.die("io", format!("writing {local_out} failed"), Exit::Error));
            }
        }
    }
    let linked = std::fs::File::open(&part)
        .and_then(|f| f.sync_all())
        .and_then(|()| std::fs::hard_link(&part, local_out))
        .and_then(|()| std::fs::remove_file(&part));
    if linked.is_err() {
        let _ = std::fs::remove_file(&part);
        return Err(ctx.die(
            "exists",
            format!("{outf} appeared while exporting; an export never overwrites a file (name another one)"),
            Exit::Usage,
        ));
    }
    Ok(())
}

fn append_chunk(chunk: &[u8], part: &str) -> R<(String, bool)> {
    let text = match std::str::from_utf8(chunk) {
        Ok(t) => t.to_string(),
        Err(e) => return Err(super::utf8_raise(3, chunk, e)),
    };
    let v = crate::out::pylib::json::loads(&text).map_err(|f| Raise::at(3, f.last_line()))?;
    let data = match ix(&v, "bytesB64", 4)? {
        b @ Value::Str(_) => crate::families::compute::writes::fetch::b64decode(&st(b))
            .map_err(|e| Raise::at(4, e))?,
        other => return Err(py::b64_type(other, 4)),
    };
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(part)
        .and_then(|mut f| f.write_all(&data))
        .map_err(|e| Raise::at(5, crate::out::pylib::json::os_error(&e, part).last_line()))?;
    let off = ix(&v, "offset", 7)?;
    let next = match off {
        Value::Int(t) => (t.parse::<i128>().unwrap_or(0) + data.len() as i128).to_string(),
        other => {
            return Err(Raise::at(
                7,
                format!(
                    "TypeError: unsupported operand type(s) for +: '{}' and 'int'",
                    opy::type_name(other)
                ),
            ))
        }
    };
    Ok((next, tr(&v, "eof", 7)?))
}

fn export_lines(pr: &mut Pr, v: &Value, local: &str, remote: bool) -> R<Exit> {
    let placed = gt_or(v, "placed", 5)?
        .cloned()
        .unwrap_or_else(|| Value::Object(Vec::new()));
    let fetch = tr(v, "fetch", 6)?;
    let where_ = if fetch {
        local.to_string()
    } else {
        sg(&placed, "path", 6)?
    };
    // `(placed.get("abs") if not remote else None) or where`.
    let abs = if remote {
        None
    } else {
        gt_or(&placed, "abs", 7)?
    };
    let full = if fetch {
        local.to_string()
    } else {
        match abs {
            Some(a) => st(a),
            None => where_.clone(),
        }
    };
    let ver = match gt(v, "version", 8)? {
        Some(x) if *x != Value::Null => format!("version {}", st(x)),
        _ => "now".to_string(),
    };
    pr.eline(&format!(
        "exported {} as {} to {where_} ({ver}, doc version {}, {} bytes)",
        st(ix(ix(v, "doc", 9)?, "path", 9)?),
        sg(v, "target", 9)?,
        sg(v, "docVersion", 9)?,
        sg(v, "size", 9)?
    ));
    let codec = gt(v, "codec", 10)?;
    if !(codec.is_none_or(|c| *c == Value::Null) || is_str(codec, "exact")) {
        pr.eline("note: rendered with today's line endings and BOM, which an outside save may have changed since that version");
    }
    let r = gt_or(v, "report", 12)?
        .cloned()
        .unwrap_or_else(|| Value::Object(Vec::new()));
    for l in rows(&r, "losses", 13)? {
        pr.eline(&format!(
            "note: lost {}: {}",
            sg(&l, "kind", 14)?,
            sg(&l, "detail", 14)?
        ));
    }
    for n in rows(&r, "notes", 15)? {
        pr.eline(&format!("note: {}", st(&n)));
    }
    pr.line(&full);
    Ok(Exit::Ok)
}

fn targets_lines(pr: &mut Pr, v: &Value) -> R<Exit> {
    let rs = rows(v, "targets", 5)?;
    pr.line(&format!(
        "{} {} {} {} OPTIONS",
        crate::families::ljust("TARGET", 14),
        crate::families::ljust("FIDELITY", 9),
        crate::families::ljust("EXT", 7),
        crate::families::ljust("RUNS", 7)
    ));
    for t in &rs {
        let mut opts = Vec::new();
        for o in rows(t, "options", 9)? {
            let k = gt_or(&o, "kind", 10)?
                .cloned()
                .unwrap_or_else(|| Value::Object(Vec::new()));
            let mut kind = gt(&k, "type", 11)?.cloned().unwrap_or(py::s(""));
            if is_str(Some(&kind), "int") {
                kind = py::s(&format!("{}..{}", sg(&k, "min", 13)?, sg(&k, "max", 13)?));
            } else if is_str(Some(&kind), "choice") {
                let vals = match gt_or(&k, "values", 15)? {
                    Some(x) => ops::join(x, "|", 15)?,
                    None => String::new(),
                };
                kind = py::s(&vals);
            }
            let dflt = match gt(&o, "default", 16)? {
                Some(x) if *x != Value::Null => format!(" (default {})", st(x)),
                _ => String::new(),
            };
            opts.push(format!("{}={}{dflt}", sg(&o, "name", 17)?, st(&kind)));
        }
        let ext = gt_or(t, "extension", 18)?.map_or_else(|| "-".to_string(), st);
        let joined = opts.join(", ");
        pr.line(&format!(
            "{} {} {} {} {}",
            left(gt(t, "id", 18)?, 14),
            left(gt(t, "fidelity", 18)?, 9),
            crate::families::ljust(&ext, 7),
            left(gt(t, "runs", 18)?, 7),
            if joined.is_empty() {
                "-"
            } else {
                joined.as_str()
            }
        ));
    }
    let wk = gt_or(v, "worker", 19)?
        .cloned()
        .unwrap_or_else(|| Value::Object(Vec::new()));
    if !tr(&wk, "available", 20)? {
        pr.eline(&format!(
            "note: the export worker is not available here ({}); only targets run by the daemon are listed",
            sg(&wk, "reason", 21)?
        ));
    }
    Ok(Exit::Ok)
}

/// Python's `s.splitlines(True)`.
fn lines_keep(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    cur.push('\n');
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => out.push(std::mem::take(&mut cur)),
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `watch` and `tail -f`: stay present and print each new version.
pub fn watch(ctx: &mut Ctx, args: &[String]) -> Step {
    let path = ctx.one_path("watch", args)?;
    let open = ctx.body_arg(&path, &[]);
    let b = ctx.call(true, "/cli/docs/open", &open)?;
    STOP.store(false, Ordering::SeqCst);
    catch_stop();
    let tier =
        match py::load(&b, 1).and_then(|v| Ok(gt(&v, "tier", 1)?.map_or_else(|| "1".into(), st))) {
            Ok(t) => t,
            Err(e) => {
                release_stop();
                return Err(Pr::default().finish(Err(e)));
            }
        };
    let r = if tier == "0" {
        watch_any(ctx, &path)
    } else {
        watch_text(ctx, &path)
    };
    release_stop();
    match r {
        Err(Exit::Ok) => {
            // Ctrl-C: leave, quietly, then exit 0.
            let body = ctx.body_arg(&path, &[]);
            let passport = ctx.client().passport().is_some();
            if let Ok(r) = ctx.client().send_json(true, "/cli/docs/leave", &body) {
                ctx.closing.answer(&r, passport);
            }
            Ok(Exit::Ok)
        }
        r => r,
    }
}

fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// One reader run on two answers, `printf '%s\n%s' A B | python3`: its
/// lines, or the end of the command.
fn two(a: &[u8], b: &[u8], f: impl FnOnce(&mut Pr, &Value, &Value) -> R<()>) -> Result<(), Exit> {
    let mut pr = Pr::default();
    let r = py::load(a, 4)
        .and_then(|x| Ok((x, py::load(b, 4)?)))
        .and_then(|(x, y)| f(&mut pr, &x, &y))
        .map(|()| Exit::Ok);
    match pr.finish(r) {
        Exit::Ok => Ok(()),
        e => Err(e),
    }
}

fn watching(body: &[u8]) -> Result<(), Exit> {
    let mut pr = Pr {
        plain: true,
        ..Pr::default()
    };
    let r = py::load(body, 1).and_then(|v| {
        pr.eline(&format!(
            "watching {} at v{} (Ctrl-C to stop)",
            st(ix(ix(&v, "doc", 1)?, "path", 1)?),
            sg(&v, "version", 1)?
        ));
        Ok(Exit::Ok)
    });
    match pr.finish(r) {
        Exit::Ok => Ok(()),
        e => Err(e),
    }
}

fn watch_text(ctx: &mut Ctx, path: &str) -> Step {
    let cat = ctx.body_arg(path, &[("argv", super::B::Argv(vec!["cat".into()]))]);
    let mut last = ctx.call(true, "/cli/docs/tool", &cat)?;
    watching(&last)?;
    loop {
        if nap(1.0) {
            return Err(Exit::Ok);
        }
        let q = ctx.query(path, &[]);
        let st_ = ctx.call(false, "/cli/docs/status", &q)?;
        let mut same = false;
        let r = py::load(&last, 3).and_then(|a| {
            let b = py::load(&st_, 3)?;
            let (va, vb) = (gt(&a, "version", 3)?, gt(&b, "version", 3)?);
            same = match (va, vb) {
                (None, None) => true,
                (Some(x), Some(y)) => ops::py_eq(x, y),
                (Some(x), None) | (None, Some(x)) => *x == Value::Null,
            };
            Ok(Exit::Ok)
        });
        if r.is_err() {
            // `if <python>; then continue; fi`: a reader that fails is
            // "they differ".
            Pr::default().finish(r);
        } else if same {
            continue;
        }
        if stopped() {
            return Err(Exit::Ok);
        }
        let body = ctx.call(true, "/cli/docs/tool", &cat)?;
        two(&last, &body, |pr, a, b| {
            let old = gt_or(a, "stdout", 5)?.map_or_else(String::new, st);
            let new = gt_or(b, "stdout", 5)?.map_or_else(String::new, st);
            let f = gt_or(b, "fence", 6)?
                .cloned()
                .unwrap_or_else(|| Value::Object(Vec::new()));
            let begin = gt_or(&f, "begin", 9)?.map_or_else(String::new, st);
            let mut who = String::new();
            const MARK: &str = "contributors since you last read:";
            for line in ops::splitlines(&begin) {
                if let Some((_, rest)) = line.split_once(MARK) {
                    who = rest.trim_matches(|c| c == ' ' || c == '┄').to_string();
                }
            }
            let (av, bv) = (sg(a, "version", 13)?, sg(b, "version", 13)?);
            pr.line(&format!(
                "── v{av} → v{bv}{}",
                if who.is_empty() {
                    String::new()
                } else {
                    format!(" by {who}")
                }
            ));
            for l in difflib::unified_diff(
                &lines_keep(&old),
                &lines_keep(&new),
                &format!("v{av}"),
                &format!("v{bv}"),
                3,
            ) {
                pr.put(&l);
                if !l.ends_with('\n') {
                    pr.put("\n");
                }
            }
            Ok(())
        })?;
        last = body;
    }
}

fn watch_any(ctx: &mut Ctx, path: &str) -> Step {
    let q = ctx.query(path, &[]);
    let mut last = ctx.call(false, "/cli/docs/status", &q)?;
    watching(&last)?;
    loop {
        if nap(1.0) {
            return Err(Exit::Ok);
        }
        let body = ctx.call(false, "/cli/docs/status", &q)?;
        two(&last, &body, |pr, a, b| {
            let (va, vb) = (gt(a, "version", 6)?, gt(b, "version", 6)?);
            let differ = match (va, vb) {
                (None, None) => false,
                (Some(x), Some(y)) => !ops::py_eq(x, y),
                (Some(x), None) | (None, Some(x)) => *x != Value::Null,
            };
            if differ {
                let c = gt_or(b, "current", 7)?
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Vec::new()));
                let extra = match gt_or(&c, "conflictCopy", 8)? {
                    Some(_) => format!(
                        " (kept the replaced one as \"{}\")",
                        st(ix(&c, "conflictCopy", 8)?)
                    ),
                    None => String::new(),
                };
                let who = gt_or(&c, "authorLabel", 10)?.or(gt(&c, "author", 10)?);
                pr.line(&format!(
                    "── v{} → v{}: {} by {}, {} bytes{extra}",
                    py::so(va),
                    py::so(vb),
                    sg(&c, "reason", 9)?,
                    py::so(who),
                    sg(&c, "size", 10)?
                ));
            }
            let la = gt_or(a, "lease", 11)?
                .cloned()
                .unwrap_or_else(|| Value::Object(Vec::new()));
            let lb = gt_or(b, "lease", 11)?
                .cloned()
                .unwrap_or_else(|| Value::Object(Vec::new()));
            let (pa, pb) = (gt(&la, "pid", 11)?.cloned(), gt(&lb, "pid", 11)?.cloned());
            let same = match (&pa, &pb) {
                (None, None) => true,
                (Some(x), Some(y)) => ops::py_eq(x, y),
                (Some(x), None) | (None, Some(x)) => *x == Value::Null,
            };
            if !same {
                let what = if pb.as_ref().is_some_and(opy::truthy) {
                    match ix(ix(b, "lease", 13)?, "label", 13)? {
                        l @ Value::Str(_) => format!("held by {}", st(l)),
                        other => {
                            return Err(Raise::at(
                                13,
                                format!(
                                    "TypeError: can only concatenate str (not \"{}\") to str",
                                    opy::type_name(other)
                                ),
                            ))
                        }
                    }
                } else {
                    "free".to_string()
                };
                pr.line(&format!("── lease: {what}"));
            }
            Ok(())
        })?;
        last = body;
    }
}
