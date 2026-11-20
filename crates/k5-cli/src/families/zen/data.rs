//! `k5 zen data <verb> …`, as the shell CLI's `cmd_zen_data` answers it
//! (`cli/src/garden-data.sh`): the person's admin of widget data on this
//! computer. Its calls, flags, bodies and readers are Garden data's
//! ([`crate::families::store::widget`]); inside a K2 terminal it sends the
//! passport and the daemon refuses (`person_only`, exit 3).
//!
//! `unadmin` and `delete` ask first unless `--yes`: one line read from
//! stdin, as `read -r` reads it (spaces and tabs at either end dropped).

use crate::exit::Exit;
use crate::families::store::widget::{
    self as gd, args, at, blob_get_file, blob_put_file, body_or_exit, export_jsonl, granted,
    lines, load, project_note, restore_body, restore_out, revoked, say, take_at, tell, usage,
    Gd, R,
};
use crate::families::{py_get, py_iter, Raise};
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::json as pyjson;
use crate::transport::Client;
use k2_client::cli_name;

#[cfg(test)]
mod tests;

/// Lines the binary leaves to the shell: `restore` whose loop meets a
/// `--<flag>` as the last word (not `--json` or `--dry-run`). Bash's
/// `shift 2 || true` shifts nothing there and the loop never ends (a bash
/// bug, reported); the binary neither copies nor fixes it.
pub fn shells(words: &[&str]) -> bool {
    if words.first() != Some(&"restore") {
        return false;
    }
    let w = &words[1..];
    let mut seen = false;
    let mut i = 0;
    while i < w.len() {
        let a = w[i];
        if a == "--json" || a == "--dry-run" {
            i += 1;
        } else if a.starts_with("--") {
            if i + 1 >= w.len() {
                return true;
            }
            i += 2;
        } else if !seen {
            seen = true;
            i += 1;
        } else {
            return false;
        }
    }
    false
}

/// `cmd_zen_data`: `words` are the words after `zen data`.
pub fn run(client: &mut Client, words: &[&str]) -> Exit {
    let sub = words.first().copied().unwrap_or("");
    let rest: Vec<String> = words.iter().skip(1).map(|w| (*w).to_string()).collect();
    let mut gd = Gd {
        client,
        json: rest.iter().any(|w| w == "--json"),
        widget: String::new(),
    };
    let r = match sub {
        "" | "help" | "--help" | "-h" => return help(false),
        "list" | "ls" => list(&mut gd, &rest),
        "show" => show(&mut gd, &rest),
        "grant" => grant(&mut gd, &rest),
        "revoke" => revoke(&mut gd, &rest),
        "admin" => admin(&mut gd, &rest),
        "unadmin" => unadmin(&mut gd, &rest),
        "delete" | "rm" => delete(&mut gd, &rest),
        "move" | "mv" => mv(&mut gd, &rest),
        "export" => export(&mut gd, &rest),
        "import" => import(&mut gd, &rest),
        "restore" => restore(&mut gd, &rest),
        _ => {
            tell(&format!(
                concat!("Error: unknown '", cli_name!(), " zen data {}'\n"),
                sub
            ));
            help(true);
            return Exit::Usage;
        }
    };
    match r {
        Ok(()) => Exit::Ok,
        Err(e) => e,
    }
}

/// `cmd_help_zen_data`, on stdout (or stderr).
fn help(to_stderr: bool) -> Exit {
    let page = crate::text::help("zen/data").expect("the zen data help page is embedded");
    if to_stderr {
        tell(page);
        return Exit::Usage;
    }
    match say(page.as_bytes()) {
        Ok(()) => Exit::Ok,
        Err(e) => e,
    }
}

/// `_gd_zen_widget VERB`: the first positional is the widget.
fn widget(gd: &mut Gd, a: &gd::Args, verb: &str) -> R<()> {
    if a.pos.is_empty() {
        return Err(usage(&format!(
            concat!("usage: ", cli_name!(), " zen data {} <widget> …"),
            verb
        )));
    }
    gd.widget = a.pos.clone();
    Ok(())
}

/// `size(n)`: bytes as the list prints them.
fn size(v: &Value, line: u32) -> Result<String, Raise> {
    let n = match v {
        Value::Int(d) => d.parse::<f64>().unwrap_or(0.0),
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        other => {
            return Err(Raise::inside(
                line,
                9,
                "size",
                format!(
                    "TypeError: '<' not supported between instances of '{}' and 'int'",
                    py::type_name(other)
                ),
            ))
        }
    };
    Ok(if n < 1024.0 {
        format!("{} B", gd::d(v, 9).map_err(|r| Raise::inside(line, 9, "size", r.exception))?)
    } else if n < 1_048_576.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / 1_048_576.0)
    })
}

/// `made`: who made a space, as the readers name it.
fn made_by(m: Option<&Value>) -> String {
    let m = match m {
        Some(v) if py::truthy(v) => v,
        _ => return "-".into(),
    };
    let kind = match m {
        Value::Object(p) => crate::families::get(p, "kind"),
        _ => None,
    };
    match kind {
        Some(Value::Str(t)) if t.as_str() == Some("owner") => "you".into(),
        Some(Value::Str(t)) if t.as_str() == Some("k2") => k2_client::product_name!().to_string(),
        Some(Value::Str(t)) if t.as_str() == Some("unclaimed") => "unsigned".into(),
        _ => match m {
            Value::Object(p) => match crate::families::get(p, "handle") {
                Some(h) if py::truthy(h) => py::str_of(h),
                _ => "-".into(),
            },
            _ => "-".into(),
        },
    }
}

fn list(gd: &mut Gd, words: &[String]) -> R<()> {
    let _ = args(gd, words)?;
    let body = gd.call(false, "/cli/zen/data/spaces", "")?;
    gd.out(&body, |b| {
        lines(b, 3, |v, o, _| {
            let spaces = at(v, "spaces", 4)?;
            let saves = at(v, "saves", 4)?;
            if !py::truthy(spaces) && !py::truthy(saves) {
                o.push_str("No widget data on this computer yet.\n");
            }
            for s in py_iter(spaces, 8)? {
                let m = py_get(&s, "madeBy", 9)?;
                let made = made_by(m);
                let flags = match py_get(&s, "orphanedAt", 11)? {
                    Some(x) if py::truthy(x) => " (no widget)",
                    _ => "",
                };
                let adm = at(&s, "admins", 12)?;
                let admins = if py::truthy(adm) {
                    format!(" admins: {}", crate::out::pylib::ops::join(adm, ", ", 12)?)
                } else {
                    String::new()
                };
                o.push_str(&format!(
                    "{} {:>8} {:>6} keys  {} grants  made by {}{}{}\n",
                    gd::left(at(&s, "widget", 13)?, 24),
                    size(at(&s, "bytes", 13)?, 13)?,
                    gd::d(at(&s, "keys", 13)?, 13)?,
                    gd::d(at(&s, "grants", 13)?, 13)?,
                    made,
                    admins,
                    flags
                ));
            }
            for s in py_iter(saves, 14)? {
                o.push_str(&format!(
                    "{} {:>8} {:>6} saved keys (private)\n",
                    gd::left(at(&s, "widget", 15)?, 24),
                    size(at(&s, "bytes", 15)?, 15)?,
                    gd::d(at(&s, "keys", 15)?, 15)?
                ));
            }
            Ok(())
        })
    })
}

fn show(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    widget(gd, &a, "show")?;
    let q = gd.q(&[]);
    let body = gd.call(false, "/cli/zen/data/show", &q)?;
    gd.out(&body, |b| {
        lines(b, 3, |v, o, _| {
            let s = at(v, "space", 4)?;
            let orphan = match py_get(s, "orphanedAt", 5)? {
                Some(x) if py::truthy(x) => " (no widget folder)",
                _ => "",
            };
            o.push_str(&format!(
                "{}: {} keys, {} bytes, rev {}{}\n",
                py::str_of(at(s, "widget", 5)?),
                gd::d(at(s, "keys", 5)?, 5)?,
                gd::d(at(s, "bytes", 5)?, 5)?,
                gd::d(at(s, "rev", 5)?, 5)?,
                orphan
            ));
            o.push_str(&format!(
                "made by: {}\n",
                made_by(py_get(v, "madeBy", 6)?)
            ));
            for g in py_iter(at(v, "grants", 8)?, 8)? {
                let gb = at(&g, "grantedBy", 9)?;
                let by = if crate::out::pylib::ops::py_eq(gb, &gd::s("owner")) {
                    String::new()
                } else {
                    format!(" (by {})", py::str_of(gb))
                };
                let prefix = at(&g, "prefix", 10)?;
                o.push_str(&format!(
                    "  {} {} {} {}{}{}\n",
                    gd::left(at(&g, "subjectKind", 10)?, 8),
                    gd::left(at(&g, "subjectName", 10)?, 24),
                    gd::left(at(&g, "level", 10)?, 5),
                    if py::truthy(prefix) {
                        py::str_of(prefix)
                    } else {
                        "(everything)".into()
                    },
                    if py::truthy(at(&g, "admin", 10)?) {
                        " ADMIN"
                    } else {
                        ""
                    },
                    by
                ));
            }
            Ok(())
        })
    })
}

fn grant(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    widget(gd, &a, "grant")?;
    let body = body_or_exit("grant", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/grant", &body)?;
    project_note(&ans, &a.project);
    gd.out(&ans, granted)
}

fn revoke(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    widget(gd, &a, "revoke")?;
    let body = body_or_exit("revoke", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/revoke", &body)?;
    gd.out(&ans, revoked)
}

fn admin(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    widget(gd, &a, "admin")?;
    let body = body_or_exit("admin", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/admin", &body)?;
    gd.out(&ans, |b| {
        lines(b, 1, |v, o, _| {
            let g = at(v, "grant", 1)?;
            let level = py::str_of(at(g, "level", 1)?);
            let prefix = at(g, "prefix", 1)?;
            o.push_str(&format!(
                "{} is now grant admin ({}) on {}. It can grant others inside that, up to {}, never itself.\n",
                py::str_of(at(g, "subjectId", 1)?),
                level,
                if py::truthy(prefix) {
                    py::str_of(prefix)
                } else {
                    "everything".into()
                },
                level
            ));
            Ok(())
        })
    })
}

/// `read -r answer || true`: one line, spaces and tabs at either end
/// dropped.
fn answer() -> String {
    let line = crate::prompt::read_line(crate::prompt::Echo::Shown, crate::prompt::AtEof::Keep);
    let text = String::from_utf8_lossy(line.expose()).into_owned();
    text.trim_matches([' ', '\t', '\n']).to_string()
}

fn unadmin(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    widget(gd, &a, "unadmin")?;
    let body = body_or_exit("unadmin", &gd.widget, &a, true)?;
    let ans = gd.call(true, "/cli/zen/data/admin/revoke", &body)?;
    // `n="$(…)"` under `set -e`: a reader that raises ends the command.
    let n = match load(&ans, 1).and_then(|v| at(&v, "cascaded", 1).map(py::str_of)) {
        Ok(n) => n.trim_end_matches('\n').to_string(),
        Err(r) => {
            tell(&r.text());
            return Err(Exit::Error);
        }
    };
    if !a.yes {
        tell(&format!("{n} grant(s) it made go with it. Go ahead? [y/N] "));
        if !matches!(answer().as_str(), "y" | "Y" | "yes") {
            tell("Nothing changed.\n");
            return Err(Exit::Error);
        }
    }
    let body = body_or_exit("unadmin", &gd.widget, &a, false)?;
    let ans = gd.call(true, "/cli/zen/data/admin/revoke", &body)?;
    gd.out(&ans, |b| {
        lines(b, 1, |v, o, _| {
            o.push_str(&format!(
                "Grant admin ended; {} grant(s) it made went with it.\n",
                py::str_of(at(v, "cascaded", 1)?)
            ));
            Ok(())
        })
    })
}

fn delete(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    widget(gd, &a, "delete")?;
    if !a.yes {
        tell(&format!(
            "Delete all of {}'s data and its grants (a snapshot is kept)? Type the widget name: ",
            gd.widget
        ));
        if answer() != gd.widget {
            tell("Nothing changed.\n");
            return Err(Exit::Error);
        }
    }
    let mut b = gd::Dict::default();
    b.set("widget", gd::s(&gd.widget));
    b.set("confirm", gd::s(&gd.widget));
    let ans = gd.call(
        true,
        "/cli/zen/data/space/delete",
        &gd::dumps_default(&b.value()),
    )?;
    gd.out(&ans, |b| {
        lines(b, 1, |v, o, _| {
            let snap = match py_get(v, "snapshot", 1)? {
                Some(x) if py::truthy(x) => format!("; snapshot {}", py::str_of(x)),
                _ => String::new(),
            };
            o.push_str(&format!(
                "Deleted {} ({} grants){}\n",
                py::str_of(at(v, "deleted", 1)?),
                py::str_of(at(v, "grantsRemoved", 1)?),
                snap
            ));
            Ok(())
        })
    })
}

fn mv(gd: &mut Gd, words: &[String]) -> R<()> {
    let a = args(gd, words)?;
    if a.pos.is_empty() || a.pos2.is_empty() {
        return Err(usage(concat!(
            "usage: ",
            cli_name!(),
            " zen data move <old> <new>"
        )));
    }
    let mut b = gd::Dict::default();
    b.set("from", gd::s(&a.pos));
    b.set("to", gd::s(&a.pos2));
    let ans = gd.call(true, "/cli/zen/data/space/move", &gd::dumps_default(&b.value()))?;
    gd.out(&ans, |b| {
        lines(b, 1, |v, o, _| {
            o.push_str(&format!(
                "Moved, with {} grant(s).\n",
                py::str_of(at(v, "grantsMoved", 1)?)
            ));
            Ok(())
        })
    })
}

/// `[0-9a-f]`.
fn is_hex(c: u8) -> bool {
    c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
}

/// The `k2blob:sha256-<64 hex>` refs an export's values name, sorted.
pub fn blob_refs(jsonl: &str) -> Result<Vec<String>, Raise> {
    fn walk(v: &Value, found: &mut std::collections::BTreeSet<String>) {
        match v {
            Value::Str(t) => {
                if let Some(s) = t.as_str() {
                    let hex = s.strip_prefix("k2blob:sha256-");
                    let hex = hex.map(|h| h.strip_suffix('\n').unwrap_or(h));
                    if hex.is_some_and(|h| h.len() == 64 && h.bytes().all(is_hex)) {
                        // `for ref in $(…)`: a trailing newline is split off.
                        found.insert(s.trim_end_matches('\n').to_string());
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|x| walk(x, found)),
            Value::Object(pairs) => pairs.iter().for_each(|(_, x)| walk(x, found)),
            _ => {}
        }
    }
    let mut found = std::collections::BTreeSet::new();
    for (n, line) in jsonl.split_inclusive('\n').enumerate() {
        if n == 0 || line.trim_matches(crate::families::py_space).is_empty() {
            continue;
        }
        let v = pyjson::loads(line).map_err(|e| Raise::at(23, e.last_line()))?;
        if let Some(x) = py_get(&v, "value", 23)? {
            walk(x, &mut found);
        }
    }
    Ok(found.into_iter().collect())
}

fn export(gd: &mut Gd, words: &[String]) -> R<()> {
    let w = words.first().cloned().unwrap_or_default();
    if w.is_empty() || w.starts_with('-') {
        return Err(usage(concat!(
            cli_name!(),
            " zen data export <widget> [--at YYYY-MM-DD] [--blobs <dir>] > data.jsonl"
        )));
    }
    let mut blobs = String::new();
    let mut rest = Vec::new();
    let mut i = 1;
    while i < words.len() {
        if words[i] == "--blobs" {
            blobs = words.get(i + 1).cloned().unwrap_or_default();
            if blobs.is_empty() {
                return Err(usage("--blobs takes a folder"));
            }
            i += 2;
        } else {
            rest.push(words[i].clone());
            i += 1;
        }
    }
    let at_ = take_at(
        concat!(
            cli_name!(),
            " zen data export <widget> [--at YYYY-MM-DD] [--blobs <dir>]"
        ),
        &rest,
    )?;
    if blobs.is_empty() {
        return export_jsonl(gd, &w, &at_);
    }
    if std::fs::create_dir_all(&blobs).is_err() {
        tell(&format!("Error: can't make '{blobs}'\n"));
        return Err(Exit::Error);
    }
    // `tmp="$(mktemp …)"; _gd_export_jsonl … > "$tmp"; cat "$tmp"`: out
    // whole, or nothing when the export stopped.
    let dir = std::env::var_os("TMPDIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into());
    match gd::make_temp(&dir.join("k2-gd-export.")) {
        Some((p, _)) => {
            let _ = std::fs::remove_file(p);
        }
        None => {
            tell("Error: can't make a temp file\n");
            return Err(Exit::Error);
        }
    }
    let mut text: Vec<u8> = Vec::new();
    gd::export_jsonl_to(gd, &w, &at_, &mut |b| {
        text.extend_from_slice(b);
        Ok(())
    })?;
    say(&text)?;
    let text = String::from_utf8_lossy(&text).into_owned();
    let refs = match blob_refs(&text) {
        Ok(r) => r,
        Err(x) => {
            tell(&x.text());
            return Err(Exit::Error);
        }
    };
    for rf in refs {
        let hash = rf.trim_start_matches("k2blob:sha256-").to_string();
        let path = format!("{blobs}/{hash}");
        if std::path::Path::new(&path).is_file() {
            continue;
        }
        blob_get_file(gd, &w, &rf, &path, "")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444));
        }
    }
    Ok(())
}

/// `cmd_zen_data_import`'s body from what stdin holds, or what its
/// Python printed (the command ends with that exit).
pub fn import_body(widget: &str, mode: &str, text: &str) -> Result<String, (String, Exit)> {
    let stop = |m: String| (format!("Error: {m}\n"), Exit::Usage);
    let s = text.trim_start_matches(crate::families::py_space);
    let rows: Vec<Value> = if s.starts_with('[') {
        match pyjson::loads(s) {
            Ok(Value::Array(rows)) => rows,
            Ok(other) => {
                return Err((
                    Raise::at(
                        8,
                        format!("TypeError: '{}' object is not iterable", py::type_name(&other)),
                    )
                    .text(),
                    Exit::Error,
                ))
            }
            Err(e) => return Err((Raise::at(8, e.last_line()).text(), Exit::Error)),
        }
    } else {
        let mut rows = Vec::new();
        for (n, line) in crate::out::pylib::ops::splitlines(text).iter().enumerate() {
            let line = line.trim_matches(crate::families::py_space);
            if line.is_empty() {
                continue;
            }
            match pyjson::loads(line) {
                Ok(v) => rows.push(v),
                Err(_) => return Err(stop(format!("line {} is not JSON", n + 1))),
            }
        }
        rows
    };
    let mut items = Vec::new();
    for (n, r) in rows.iter().enumerate() {
        let n = n + 1;
        let Value::Object(pairs) = r else {
            return Err(stop(format!("row {n} is not a JSON object")));
        };
        let has = |k: &str| crate::families::get(pairs, k).is_some();
        if has("space") && !has("key") {
            continue;
        }
        if !has("key") || !has("value") {
            return Err(stop(format!("row {n} needs \"key\" and \"value\"")));
        }
        let mut it = gd::Dict::default();
        it.set("key", crate::families::get(pairs, "key").cloned().unwrap_or(Value::Null));
        it.set("value", crate::families::get(pairs, "value").cloned().unwrap_or(Value::Null));
        items.push(it.value());
    }
    let mut b = gd::Dict::default();
    b.set("widget", gd::s(widget));
    b.set("mode", gd::s(mode));
    b.set("items", Value::Array(items));
    Ok(gd::dumps_compact(&b.value(), false))
}

fn import(gd: &mut Gd, words: &[String]) -> R<()> {
    let w = words.first().cloned().unwrap_or_default();
    let usage_line = concat!(
        cli_name!(),
        " zen data import <widget> [--replace] [--blobs <dir>] < data.jsonl"
    );
    if w.is_empty() || w.starts_with('-') {
        return Err(usage(usage_line));
    }
    let mut mode = "merge";
    let mut blobs = String::new();
    let mut i = 1;
    while i < words.len() {
        match words[i].as_str() {
            "--replace" => mode = "replace",
            "--merge" => mode = "merge",
            "--blobs" => {
                blobs = words.get(i + 1).cloned().unwrap_or_default();
                if !std::path::Path::new(&blobs).is_dir() {
                    return Err(usage("--blobs takes a folder of files named by their sha256"));
                }
                i += 1;
            }
            "--json" => {}
            x => {
                return Err(usage(&format!(
                    concat!(
                        "unexpected '{}' (",
                        cli_name!(),
                        " zen data import <widget> [--replace] < data.jsonl)"
                    ),
                    x
                )))
            }
        }
        i += 1;
    }
    if crate::prompt::stdin_is_tty() {
        return Err(usage(&format!(
            concat!("pipe the data in: ", cli_name!(), " zen data import {} < data.jsonl"),
            w
        )));
    }
    let mut input = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut input);
    let body = match import_body(&w, mode, &String::from_utf8_lossy(&input)) {
        Ok(b) => b,
        Err((said, exit)) => {
            tell(&said);
            return Err(exit);
        }
    };
    if !blobs.is_empty() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&blobs)
            .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        // The shell's glob: sorted, no dot files.
        files.retain(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'))
        });
        files.sort();
        for f in files {
            if !f.is_file() {
                continue;
            }
            let base = f
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if base.len() != 64 || !base.bytes().all(is_hex) {
                continue;
            }
            let path = format!("{blobs}/{base}");
            let ans = blob_put_file(gd, &w, &path, "", "")?;
            let rf = match load(&ans, 1).and_then(|v| at(&v, "ref", 1).map(py::str_of)) {
                Ok(r) => r,
                Err(x) => {
                    tell(&x.text());
                    return Err(Exit::Error);
                }
            };
            if rf.trim_end_matches('\n') != format!("k2blob:sha256-{base}") {
                tell(&format!(
                    "Error: {base} isn't named by its sha256; nothing was imported\n"
                ));
                return Err(Exit::Error);
            }
        }
    }
    let ans = gd.call(true, "/cli/zen/data/import", &body)?;
    if gd.json {
        let mut b = ans;
        b.push(b'\n');
        return say(&b);
    }
    gd::reader(lines(&ans, 3, |v, o, _| {
        o.push_str(&format!(
            "Imported {} key(s) into {} at rev {}; {} deleted. A snapshot was taken first.\n",
            gd::d(at(v, "written", 4)?, 4)?,
            w,
            gd::d(at(v, "rev", 4)?, 4)?,
            gd::d(at(v, "deleted", 4)?, 4)?
        ));
        Ok(())
    }))
}

fn restore(gd: &mut Gd, words: &[String]) -> R<()> {
    let mut w = String::new();
    let mut pass = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        if a == "--json" {
            gd.json = true;
            i += 1;
        } else if a.starts_with("--") {
            pass.push(a.to_string());
            if a == "--dry-run" {
                i += 1;
                continue;
            }
            // `pass+=("${2-}"); shift 2 || true`: an empty value at the end.
            pass.push(words.get(i + 1).cloned().unwrap_or_default());
            i += 2;
        } else if w.is_empty() {
            w = a.to_string();
            i += 1;
        } else {
            return Err(usage(&format!("unexpected '{a}'")));
        }
    }
    if w.is_empty() {
        return Err(usage(concat!(
            "usage: ",
            cli_name!(),
            " zen data restore <widget> (--rev N | --at 2026-10-26T17:30 | --snapshot <ref>) [--prefix p/]… [--except r/]… [--dry-run]"
        )));
    }
    let body = restore_body("restore", &w, &pass).map_err(|said| {
        tell(&said);
        Exit::Usage
    })?;
    let ans = gd.call(true, "/cli/zen/data/restore", &body)?;
    gd.out(&ans, restore_out)
}
