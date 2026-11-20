//! `k2 vault carry export|import` (`cli/src/migrate.sh`, `cmd_vault`):
//! secrets sealed for another vault, written to a file only that vault
//! can open, and that file read back on the new box. Both go through
//! `_vault_call` (the agent's channel); the file holds only the sealed
//! bytes.

use super::py::{self, get, idx, items, left, render, s, t, Lines, R};
use super::verbs::{b64, b64_decode, echo, kv, obj, print_body, usage, Opts};
use super::Ctx;
use crate::exit::Exit;
use crate::families::Raise;
use crate::out;
use crate::out::json::Value;
use k2_client::brand::CLI_NAME as CLI;

/// `carry export` or `carry import`, or the usage line.
pub fn carry(ctx: &mut Ctx, o: &Opts) -> Result<Exit, Exit> {
    match o.pos.first().map(String::as_str) {
        Some("export") => export(ctx, o),
        Some("import") => import(ctx, o),
        _ => Err(usage(format!("usage: {CLI} vault carry export|import"))),
    }
}

/// Python's exception for an `open()` that failed.
fn os_error(e: &std::io::Error, path: &str) -> Raise {
    let name = match e.kind() {
        std::io::ErrorKind::AlreadyExists => "FileExistsError",
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    };
    let errno = e.raw_os_error().unwrap_or(0);
    let text = e.to_string();
    let text = text.split(" (os error").next().unwrap_or("");
    Raise::at(1, format!("{name}: [Errno {errno}] {text}: '{path}'"))
}

/// `open(path, "xb")` under `umask 077`: a new file only its owner reads.
#[cfg(unix)]
fn create_private(path: &str) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &str) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// `base64.b64decode(x)` of a JSON value.
fn b64decode(v: &Value) -> R<Vec<u8>> {
    match v {
        Value::Str(_) => {
            let text = s(v);
            if !text.is_ascii() {
                return Err(Raise::at(
                    1,
                    "ValueError: string argument should contain only ASCII characters",
                ));
            }
            b64_decode(&text).ok_or_else(|| Raise::at(1, "binascii.Error: Incorrect padding"))
        }
        other => Err(Raise::at(
            1,
            format!(
                "TypeError: argument should be a bytes-like object or ASCII string, not '{}'",
                crate::out::py::type_name(other)
            ),
        )),
    }
}

/// `k2 vault carry export --recipient <age1…> --target <vault-id>
/// --workspace <ws> [--server NAME] --out <file>`.
fn export(ctx: &mut Ctx, o: &Opts) -> Result<Exit, Exit> {
    if o.recipient.is_empty() || o.target.is_empty() || o.out_file.is_empty() {
        return Err(usage(format!("usage: {CLI} vault carry export --recipient <age1…> --target <vault-id> --workspace <ws> [--server NAME] --out <file>")));
    }
    let out_file = o.out_file.clone();
    if std::path::Path::new(&out_file).exists() {
        return Err(usage(format!("usage: {out_file} already exists")));
    }
    // `"${carry_ws[*]}"` joined by \x1f, split on it again, empties out.
    let list = |v: &[String]| {
        Value::Array(
            v.iter()
                .flat_map(|x| x.split('\x1f'))
                .filter(|x| !x.is_empty())
                .map(py::text)
                .collect(),
        )
    };
    let mut body = vec![
        kv("targetRecipient", py::text(&o.recipient)),
        kv("targetVaultId", py::text(&o.target)),
        kv("ownBoxMove", Value::Bool(o.own_box_move)),
        kv("workspaces", list(&o.workspaces)),
        kv("server", list(&o.servers)),
    ];
    if !o.target_ws.is_empty() {
        body.push(kv("targetWorkspaceId", py::text(&o.target_ws)));
    }
    let r = ctx.call(true, "carry/export", &obj(body))?;
    // `( umask 077; … python3 … ) || exit 1`: the file first, then the
    // sealed bytes in it, then the lines on stderr.
    let shown = render(&r.body, |v, l| {
        let mut f = create_private(&out_file).map_err(|e| os_error(&e, &out_file))?;
        let bytes = b64decode(&idx(v, "carryB64")?)?;
        // A write that fails is no exit to the shell's Python either way.
        let _ = std::io::Write::write_all(&mut f, &bytes);
        l.e(format!(
            "{} secret(s) sealed for {}:",
            py::d(&idx(v, "count")?)?,
            s(&idx(v, "targetFingerprint")?)
        ));
        for n in items(&idx(v, "secrets")?)? {
            l.e(py::cat("  ", &n)?);
        }
        Ok(0)
    });
    let e = shown.print();
    if e != Exit::Ok {
        return Err(Exit::Error);
    }
    Ok(echo(&format!(
        "wrote {out_file} (only the target vault can open it)"
    )))
}

/// `k2 vault carry import <file> [--workspace <id>] [--apply]`: a dry run
/// unless `--apply`.
fn import(ctx: &mut Ctx, o: &Opts) -> Result<Exit, Exit> {
    let f = o.pos.get(1).cloned().unwrap_or_default();
    if f.is_empty() || !std::path::Path::new(&f).is_file() {
        return Err(usage(format!(
            "usage: {CLI} vault carry import <file> [--workspace <id>] [--apply]"
        )));
    }
    if o.workspaces.len() > 1 {
        return Err(usage(format!(
            "usage: {CLI} vault carry import takes one --workspace <id>"
        )));
    }
    // `ib="$(python3 …)"`: a file Python can't read is its traceback.
    let ib = match std::fs::read(&f) {
        Ok(bytes) => {
            let mut b = vec![
                kv("carryB64", py::text(&b64(&bytes))),
                kv("apply", Value::Bool(o.apply)),
            ];
            if let Some(ws) = o.workspaces.first() {
                b.push(kv("workspace", py::text(ws)));
            }
            obj(b)
        }
        Err(e) => {
            // `ib="$(python3 …)"` under `set -e`: the command ends.
            out::stderr_line(os_error(&e, &f).text().trim_end_matches('\n'));
            return Err(Exit::Error);
        }
    };
    let r = ctx.call(true, "carry/import", &ib)?;
    if o.json {
        return Ok(print_body(&r));
    }
    Ok(render(&r.body, import_printer).print())
}

fn import_printer(v: &Value, l: &mut Lines) -> R<i32> {
    let joined = |x: &Value| -> R<String> {
        let mut parts = Vec::new();
        for (i, p) in items(x)?.iter().enumerate() {
            match p {
                Value::Str(_) => parts.push(s(p)),
                other => {
                    return Err(Raise::at(
                        1,
                        format!(
                            "TypeError: sequence item {i}: expected str instance, {} found",
                            crate::out::py::type_name(other)
                        ),
                    ))
                }
            }
        }
        Ok(parts.join(", "))
    };
    if t(&get(v, "dryRun")?) {
        let own = if t(&get(v, "ownBoxMove")?) {
            ", owner move"
        } else {
            ""
        };
        l.p(format!(
            "from vault {} (key {}){own}:",
            s(&idx(v, "sourceVaultId")?),
            s(&idx(v, "sourceKeyFingerprint")?)
        ));
        if t(&get(v, "madeForWorkspaceId")?) {
            l.p(format!(
                "made for workspace {}",
                s(&idx(v, "madeForWorkspaceId")?)
            ));
        }
        for x in items(&idx(v, "secrets")?)? {
            let dest = if t(&get(&x, "skip")?) {
                "-> skipped".to_string()
            } else if t(&get(&x, "targetWorkspaceId")?) {
                let tw = get(&x, "targetWorkspace")?;
                let tw = if t(&tw) { tw } else { py::text("?") };
                format!("-> {} ({})", s(&tw), s(&idx(&x, "targetWorkspaceId")?))
            } else {
                "-> server".to_string()
            };
            let scope = idx(&x, "scope")?;
            let name = idx(&x, "name")?;
            let hosts = joined(&idx(&x, "hosts")?)?;
            let hosts = if hosts.is_empty() { "-".into() } else { hosts };
            l.p(format!(
                "  {} {} {} {hosts}",
                left(&scope, 24),
                left(&name, 28),
                left(&py::text(&dest), 40)
            ));
            if t(&get(&x, "skip")?) {
                l.p(py::cat("      ", &idx(&x, "skip")?)?);
            }
        }
        l.p("nothing changed yet; add --apply to put these in this vault");
    } else {
        let imported = joined(&idx(v, "imported")?)?;
        l.p(format!(
            "imported: {}",
            if imported.is_empty() {
                "none"
            } else {
                imported.as_str()
            }
        ));
        for x in items(&idx(v, "skipped")?)? {
            l.p(py::cat("skipped:  ", &x)?);
        }
    }
    Ok(0)
}
