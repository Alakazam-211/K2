//! The tool calls (`_doc_tool`, `_doc_blob_tool`, `_doc_node_claim`), a
//! whole file's write (`write --base`), and `_doc_print_tool`'s reader.

use super::print::{eq, ttl_ms, v2};
use super::py::{self, gt, gt_or, ix, st, tr, Pr, R};
use super::{m, w, Ctx, Step, B};
use crate::exit::Exit;
use crate::families::{sim::is_tty, Raise};
use crate::out::json::{Text, Value};
use crate::out::py as opy;
use crate::out::pylib::json as pyjson;
use k2_client::brand::CLI_NAME;

/// The C library's words for an OS error (Python's `e.strerror`).
pub fn strerror(e: &std::io::Error) -> String {
    #[cfg(unix)]
    if let Some(n) = e.raw_os_error() {
        // SAFETY: strerror returns a NUL-terminated string the C library
        // owns; it is copied at once.
        let p = unsafe { libc::strerror(n) };
        if !p.is_null() {
            // SAFETY: as above.
            return unsafe { std::ffi::CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned();
        }
    }
    e.to_string()
}

/// `_DOC_TOOL_PY`'s `die`: always its `Error:` line, then the JSON.
fn tool_die(msg: &str, rc: Exit) -> Exit {
    super::super::sim::Die {
        exit: rc,
        json: false,
        ..super::super::sim::die("usage", msg)
    }
    .print()
}

/// `_DOC_TOOL_PY`: the tool call's body, or its `die`.
fn tool_body(ctx: &Ctx, path: &str, argv: &[String]) -> Result<String, Exit> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        if a == "--" {
            out.extend(argv[i..].iter().cloned());
            break;
        }
        let (flag, fp) = if a == "--old-file" || a == "--new-file" {
            let Some(fp) = argv.get(i + 1) else {
                return Err(tool_die(&format!("{a} needs a file"), Exit::Usage));
            };
            i += 2;
            (&a[..a.len() - 5], fp.clone())
        } else if a.starts_with("--old-file=") || a.starts_with("--new-file=") {
            let (k, fp) = a.split_once('=').unwrap_or((a, ""));
            i += 1;
            (&k[..k.len() - 5], fp.to_string())
        } else {
            out.push(a.to_string());
            i += 1;
            continue;
        };
        let raw = std::fs::read(&fp).map_err(|e| {
            tool_die(
                &format!("{flag}-file {fp}: {}", strerror(&e)),
                Exit::NotFound,
            )
        })?;
        match String::from_utf8(raw) {
            Ok(t) => {
                out.push(flag.to_string());
                out.push(t);
            }
            Err(_) => {
                return Err(tool_die(
                    &format!("{flag}-file {fp} is not UTF-8 text"),
                    Exit::Usage,
                ))
            }
        }
    }
    let verb = out[0].clone();
    let rest = &out[1..];
    let opts: &[String] = match rest.iter().position(|a| a == "--") {
        Some(n) => &rest[..n],
        None => rest,
    };
    let exact = opts.iter().any(|a| {
        a == "--old" || a == "--new" || a.starts_with("--old=") || a.starts_with("--new=")
    });
    let reads = matches!(
        verb.as_str(),
        "diff" | "patch" | "apply" | "append" | "insert" | "replace-section"
    ) || (verb == "replace" && !exact)
        || opts.iter().any(|a| a == "-");
    let mut d: Vec<(Text, Value)> = vec![(
        Text::from("argv"),
        Value::Array(out.iter().map(|x| py::s(x)).collect()),
    )];
    let mut put = |k: &str, v: Value| d.push((Text::from(k), v));
    if ctx.json {
        put("json", Value::Bool(true));
    }
    if !ctx.ws.is_empty() {
        put("workspace", py::s(&ctx.ws));
    }
    if !path.is_empty() {
        put("path", py::s(path));
    }
    if !ctx.ob.is_empty() {
        put("onBehalfOf", py::s(&ctx.ob));
    }
    if reads {
        if let Some(t) = &ctx.text {
            put("stdin", py::s(t));
        } else if is_tty(0) {
            return Err(tool_die(
                &format!(
                    "{CLI_NAME} doc {verb} reads its text from stdin (pipe it in) or --text \"...\""
                ),
                Exit::Usage,
            ));
        } else {
            let mut b = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut b);
            match String::from_utf8(b) {
                Ok(t) => put("stdin", py::s(&t)),
                Err(_) => {
                    return Err(tool_die(
                        &format!("{CLI_NAME} doc {verb}: stdin is not UTF-8 text"),
                        Exit::Usage,
                    ))
                }
            }
        }
    }
    Ok(py::dumps(&Value::Object(d)))
}

/// `_doc_tool VERB ARGS...`: the last word is the doc; the rest go to the
/// tool as given.
pub fn tool(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let args = ctx.parse(words)?;
    let Some(last) = args.last() else {
        return Err(ctx.usage(format!(
            "name the doc: {CLI_NAME} doc {verb} [flags] <file.md>"
        )));
    };
    if last.starts_with('-') {
        return Err(ctx.usage(format!(
            "the doc's path comes last: {CLI_NAME} doc {verb} [flags] <file.md>"
        )));
    }
    let mut argv = vec![verb.to_string()];
    argv.extend(args[..args.len() - 1].iter().cloned());
    let path = ctx.path(last);
    ctx.tmp()?;
    let body = tool_body(ctx, &path, &argv)? + "\n";
    let ans = ctx.xfer(true, "/cli/docs/tool", &body)?;
    Ok(print_tool(ctx, &ans))
}

/// The kit's flags that take a value (`_DOC_KIT_VALUE_FLAGS`).
const KIT_VALUE_FLAGS: &[&str] = &[
    "--body",
    "--note",
    "--hint",
    "--mention",
    "--proposal",
    "--cursor",
    "--kind",
    "--under",
    "--version",
    "--depth",
    "--ttl",
    "--id",
    "--node",
    "--on",
    "--since",
    "--after",
    "--limit",
    "--n",
];

/// `_doc_blob_tool VERB ARGS...`: the doc is the last word that isn't a
/// flag's value; K2's own flags are K2's, the rest the kit's, in order.
pub fn blob(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let (mut k2args, mut toks) = (Vec::new(), Vec::<String>::new());
    let mut last: Option<usize> = None;
    let mut budget = String::new();
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
            "--fence" | "--no-fence" => k2args.push(a.to_string()),
            "--read-budget" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(ctx.usage("--read-budget needs a number of bytes"));
                };
                budget = v.clone();
                i += 2;
                continue;
            }
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
            _ if a.starts_with("--read-budget=") => {
                budget = a["--read-budget=".len()..].to_string()
            }
            _ if a.starts_with('-') => {
                if KIT_VALUE_FLAGS.contains(&a) {
                    let Some(v) = words.get(i + 1) else {
                        return Err(ctx.usage(format!("{a} needs a value ({CLI_NAME} doc --help)")));
                    };
                    toks.push(a.to_string());
                    toks.push(v.clone());
                    i += 2;
                    continue;
                }
                toks.push(a.to_string());
            }
            _ => {
                toks.push(a.to_string());
                last = Some(toks.len() - 1);
            }
        }
        i += 1;
    }
    let Some(last) = last else {
        return Err(ctx.usage(format!(
            "name the doc last: {CLI_NAME} doc {verb} [flags] <file>"
        )));
    };
    if !budget.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ctx.usage("--read-budget takes a number of bytes (at least 16384)"));
    }
    let path = toks.remove(last);
    ctx.parse(&k2args)?;
    let path = ctx.path(&path);
    let mut argv = vec![verb.to_string()];
    argv.extend(toks);
    let mut kv: Vec<(&str, B)> = vec![("argv", B::Argv(argv))];
    if !budget.is_empty() {
        kv.push(("readBudget", m(format!("@int:{budget}"))));
    }
    ctx.tmp()?;
    // The body goes from a file `print` wrote: a newline after it. With
    // none, the empty file goes (the shell checks nothing there).
    let body = match ctx.body(&path, &kv) {
        Ok(b) => b + "\n",
        Err(e) => {
            e.print();
            String::new()
        }
    };
    let ans = ctx.xfer(true, "/cli/docs/tool", &body)?;
    Ok(print_tool(ctx, &ans))
}

/// `_doc_node_claim`: `--ttl` as milliseconds, then the kit's `claim`.
pub fn node_claim(ctx: &mut Ctx, words: &[String]) -> Step {
    let mut args = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        if a == "--ttl" || a.starts_with("--ttl=") {
            let ttl = if a == "--ttl" {
                let t = v2(words, i)?;
                i += 2;
                t
            } else {
                i += 1;
                a["--ttl=".len()..].to_string()
            };
            let Some(ms) = ttl_ms(&ttl) else {
                return Err(ctx.usage("--ttl takes a duration: 30s, 5m, or milliseconds"));
            };
            args.push("--ttl".to_string());
            args.push(ms.to_string());
            continue;
        }
        args.push(a.to_string());
        i += 1;
    }
    blob(ctx, "claim", &args)
}

/// `write --base <version>|live <file>`: the whole new content as base64.
pub fn write(ctx: &mut Ctx, words: &[String]) -> Step {
    let mut base = String::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--base" => {
                base = v2(words, i)?;
                i += 2;
                continue;
            }
            "--file" => {
                ctx.file = v2(words, i)?;
                i += 2;
                continue;
            }
            _ => match eq(a, "--base") {
                Some(v) => base = v.to_string(),
                None => rest.push(a.to_string()),
            },
        }
        i += 1;
    }
    if base.is_empty() {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc write --base <version>|live <file.md> < text (read the doc first: {CLI_NAME} doc cat)"
        )));
    }
    let args = ctx.parse(&rest)?;
    let path = ctx.one_path("write", &args)?;
    if ctx.text.is_none() && ctx.file.is_empty() && is_tty(0) {
        return Err(ctx.usage(format!(
            "{CLI_NAME} doc write reads the whole new content from stdin, --file or --text"
        )));
    }
    if !ctx.file.is_empty() && !std::path::Path::new(&ctx.file).is_file() {
        return Err(ctx.die(
            "usage",
            format!("--file {}: no such file", ctx.file),
            Exit::NotFound,
        ));
    }
    ctx.tmp()?;
    let body = match ctx.body(&path, &[("base", w(base)), ("bytesB64", m("@b64"))]) {
        Ok(b) => b + "\n",
        Err(e) => {
            e.print();
            return Err(ctx.usage("couldn't read the new content"));
        }
    };
    let ans = ctx.xfer(true, "/cli/docs/write", &body)?;
    let held = super::substituted(&ans);
    Ok(ctx.json_or(&held, |pr, v| {
        let r = gt_or(v, "result", 4)?
            .cloned()
            .unwrap_or_else(|| Value::Object(Vec::new()));
        let p = || -> R<String> { Ok(st(ix(ix(v, "doc", 6)?, "path", 6)?)) };
        if gt(v, "tier", 5)?
            .is_some_and(|t| crate::out::pylib::ops::py_eq(t, &Value::Int("0".into())))
        {
            if tr(&r, "changed", 6)? {
                pr.eline(&format!(
                    "wrote {} ({} bytes) · v{}",
                    p()?,
                    py::sg(&r, "size", 7)?,
                    py::sg(&r, "version", 7)?
                ));
            } else {
                pr.eline(&format!(
                    "{} already holds those bytes · v{} (nothing new recorded)",
                    p()?,
                    py::sg(&r, "version", 9)?
                ));
            }
            return Ok(Exit::Ok);
        }
        let added = gt(&r, "linesAdded", 11)?.map_or_else(|| "0".to_string(), st);
        let removed = gt(&r, "linesRemoved", 11)?.map_or_else(|| "0".to_string(), st);
        pr.eline(&format!(
            "wrote {} (+{added} -{removed}) · v{}",
            p()?,
            py::sg(&r, "version", 11)?
        ));
        if tr(v, "warning", 12)? {
            match ix(v, "warning", 13)? {
                x @ Value::Str(_) => pr.eline(&format!("Warning: {}", st(x))),
                other => return Err(concat_raise(13, other)),
            }
        }
        if tr(v, "notes", 14)? {
            let n = py::text(ix(v, "notes", 15)?, "rstrip", 15)?;
            pr.eline(n.trim_end_matches('\n'));
        }
        if let Some(a) = gt_or(v, "approval", 16)? {
            pr.eline(&format!(
                "on behalf of {}: {}",
                py::sg(a, "onBehalfOf", 18)?,
                py::sg(a, "state", 18)?
            ));
        }
        Ok(Exit::Ok)
    }))
}

fn concat_raise(line: u32, other: &Value) -> Raise {
    Raise::at(
        line,
        format!(
            "TypeError: can only concatenate str (not \"{}\") to str",
            opy::type_name(other)
        ),
    )
}

/// `open(file, encoding="utf-8").read()`: text mode turns `\r\n` and
/// `\r` into `\n`; a byte that isn't UTF-8 raises.
fn read_text(bytes: &[u8], line: u32) -> R<String> {
    let t = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => return Err(super::utf8_raise(line, bytes, e)),
    };
    Ok(t.replace("\r\n", "\n").replace('\r', "\n"))
}

/// The answer's lines: approval and the read-version note.
fn tail_notes(pr: &mut Pr, v: &Value, line: u32) -> R<()> {
    if gt(v, "ifVersion", line)?.is_some_and(|x| *x != Value::Null) && tr(v, "changed", line)? {
        pr.eput(&format!(
            "(checked against v{}, the version you last read)\n",
            st(ix(v, "ifVersion", line)?)
        ));
    }
    if let Some(a @ Value::Object(_)) = gt(v, "approval", line + 1)? {
        pr.eput(&format!(
            "on behalf of {}: {}\n",
            py::sg(a, "onBehalfOf", line + 2)?,
            py::sg(a, "state", line + 2)?
        ));
    }
    Ok(())
}

/// `_doc_print_tool`: a tool answer's bytes on stdout, the fence and the
/// notes on stderr, the tool's exit.
pub fn print_tool(ctx: &Ctx, ans: &[u8]) -> Exit {
    let mut pr = Pr::default();
    let r = if ctx.json {
        tool_json(&mut pr, ans)
    } else {
        let fence = ctx.fence.unwrap_or(ctx.in_cell);
        tool_lines(&mut pr, ans, fence, is_tty(1))
    };
    pr.finish(r)
}

fn tool_json(pr: &mut Pr, ans: &[u8]) -> R<Exit> {
    let raw = read_text(ans, 3)?;
    let v = match pyjson::loads(&raw) {
        Ok(v) => v,
        Err(f) if matches!(f.class, "JSONDecodeError" | "ValueError") => {
            pr.put(&raw);
            pr.put("\n");
            return Ok(Exit::Error);
        }
        Err(f) => return Err(Raise::at(5, f.last_line())),
    };
    let r = match &v {
        Value::Object(pairs) => crate::families::get(pairs, "result"),
        _ => None,
    };
    if !matches!(r, Some(Value::Object(_))) {
        pr.put(&raw);
        pr.put("\n");
        return match &v {
            Value::Object(_) => {
                py::exit_int(gt(&v, "exit", 12)?.unwrap_or(&Value::Int("1".into())), 12)
            }
            _ => Ok(Exit::Error),
        };
    }
    let r = r.cloned().unwrap_or(Value::Null);
    let out = py::text(ix(&v, "stdout", 13)?, "rstrip", 13)?;
    pr.put(out.trim_end_matches('\n'));
    pr.put("\n");
    tail_notes(pr, &v, 14)?;
    py::exit_int(ix(&r, "exit", 19)?, 19)
}

fn tool_lines(pr: &mut Pr, ans: &[u8], fence: bool, tty: bool) -> R<Exit> {
    let raw = read_text(ans, 3)?;
    let v = pyjson::loads(&raw).map_err(|f| Raise::at(3, f.last_line()))?;
    let f = if fence {
        gt(&v, "fence", 5)?.cloned()
    } else {
        None
    };
    let out = gt(&v, "stdout", 6)?.cloned();
    let is_text = matches!(out, Some(Value::Str(_)));
    let data: Vec<u8> = match &out {
        Some(o @ Value::Str(_)) => st(o).into_bytes(),
        _ => match gt_or(&v, "stdoutB64", 7)? {
            None => Vec::new(),
            Some(b @ Value::Str(_)) => crate::families::compute::writes::fetch::b64decode(&st(b))
                .map_err(|e| Raise::at(7, e))?,
            Some(other) => return Err(py::b64_type(other, 7)),
        },
    };
    if tty && (!is_text || data.contains(&0)) {
        let hint = format!(
            "{} is binary ({} bytes); pipe it, or write it to a file: {CLI_NAME} doc show --version V --out FILE",
            st(ix(ix(&v, "doc", 9)?, "path", 9)?),
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
    let f = f.filter(opy::truthy);
    if let Some(f) = &f {
        let begin = gt(f, "begin", 14)?.cloned().unwrap_or(py::s(""));
        pr.eput(&py::write_arg(&begin, 14)?);
    }
    pr.out.extend_from_slice(&data);
    if let Some(f) = &f {
        if !data.is_empty() && !data.ends_with(b"\n") {
            pr.eput("\n");
        }
        let end = gt(f, "end", 20)?.cloned().unwrap_or(py::s(""));
        pr.eput(&py::write_arg(&end, 20)?);
    }
    if let Some(e) = gt_or(&v, "stderr", 21)? {
        let e = py::text(e, "rstrip", 23)?;
        pr.eput(&format!("{}\n", e.trim_end_matches('\n')));
    }
    for k in ["forEdit", "notes"] {
        if let Some(x) = gt_or(&v, k, 25)? {
            let x = py::text(x, "rstrip", 26)?;
            pr.eput(&format!("{}\n", x.trim_end_matches('\n')));
        }
    }
    tail_notes(pr, &v, 27)?;
    py::exit_int(gt(&v, "exit", 32)?.unwrap_or(&Value::Int("1".into())), 32)
}
