//! A folder the daemon reads, as the shell CLI answers it: `app init`
//! (`cmd_app_init`, `cli/src/app-init.sh`: a template's files written into
//! a folder) and `app doctor --store` (`_app_store_doctor`,
//! `cli/src/app-doctor.sh`: a Tauri App's store check). Each is the
//! shell's word loop, the folder as `cd <folder> && pwd -P` gives it, one
//! POST (`_app_rt_post`), `--json` as it came, else the shell's Python
//! line for line.
//!
//! The shell answers these itself (`None`): `-h` or `--help` anywhere,
//! `app doctor` without `--store` (the box and reach doctors), a folder
//! `CDPATH` moves `cd` to ([`crate::families::sim::cd_moved`]), and a folder `cd` can't enter or whose path is not UTF-8 or ends
//! in a newline.

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_iter, py_len, Raise, Shown};
use crate::out::json::Value;
use crate::out::pylib::ops::py_eq;
use k2_client::cli_name;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `init --template T [<folder>] …`.
    Init {
        folder: String,
        template: String,
        name: String,
        app: String,
        voice: bool,
        dry: bool,
        json: bool,
    },
    /// `doctor --store [<folder>] [--voice|--no-voice]`: `voice` is `"1"`,
    /// `"0"` or `""`.
    Store {
        folder: String,
        voice: &'static str,
        json: bool,
    },
}

fn help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `(cd "$p" && pwd -P)` for a folder `[ -d ]` found: `None` where the
/// binary can't be sure to get what the shell gets.
fn abs(folder: &str) -> Option<String> {
    if crate::families::sim::cd_moved(folder) {
        return None;
    }
    let p = std::fs::canonicalize(folder).ok()?;
    // `cd` needs search permission on the folder.
    std::fs::read_dir(&p).ok()?;
    let s = p.to_str()?.to_string();
    if s.ends_with('\n') {
        return None;
    }
    Some(s)
}

/// The words after `init`. `None`: the shell's.
pub fn parse_init(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    const HELP: &str = concat!(" (", cli_name!(), " app init --help)");
    const HEAD: &str = concat!(cli_name!(), " app init: ");
    let (mut template, mut folder, mut name, mut app) =
        (String::new(), String::new(), String::new(), String::new());
    let (mut voice, mut dry, mut json) = (false, false, false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--template" => Some(&mut template),
            "--name" => Some(&mut name),
            "--app" => Some(&mut app),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = rest.get(1).copied().unwrap_or("").to_string();
            if rest.len() < 2 {
                return Some(Err(Stop::need_value(w)));
            }
            rest = &rest[2..];
            continue;
        }
        if let Some(v) = w.strip_prefix("--template=") {
            template = v.to_string();
        } else if let Some(v) = w.strip_prefix("--name=") {
            name = v.to_string();
        } else if let Some(v) = w.strip_prefix("--app=") {
            app = v.to_string();
        } else {
            match w {
                "--voice" => voice = true,
                "--dry-run" => dry = true,
                "--json" => json = true,
                _ if w.starts_with('-') => {
                    return Some(Err(Stop::usage(format!("Unknown flag: {w}{HELP}"))))
                }
                _ if folder.is_empty() => folder = w.to_string(),
                _ => return Some(Err(Stop::usage(format!("Unexpected argument: {w}{HELP}")))),
            }
        }
        rest = &rest[1..];
    }
    let stop = |line: String| Some(Err(Stop::usage(line)));
    match template.as_str() {
        "tauri" | "static" | "node-api" => {}
        "" => {
            return stop(format!(
                "{HEAD}--template is required: tauri, static or node-api{HELP}"
            ))
        }
        t => {
            return stop(format!(
                "{HEAD}--template is tauri, static or node-api, not '{t}'"
            ))
        }
    }
    if template != "tauri" && (!app.is_empty() || voice) {
        return stop(format!(
            "{HEAD}--app and --voice go with --template tauri, not --template {template}"
        ));
    }
    if !app.is_empty() && !name.is_empty() {
        return stop(format!(
            "{HEAD}pass --app or --name, not both (--app names the App the shell updates through)"
        ));
    }
    if folder.is_empty() {
        folder = ".".into();
    }
    if !std::path::Path::new(&folder).is_dir() {
        return stop(format!("{HEAD}{folder} is not a folder"));
    }
    let folder = abs(&folder)?;
    Some(Ok(Ask::Init {
        folder,
        template,
        name,
        app,
        voice,
        dry,
        json,
    }))
}

/// The words after `doctor`. `None`: the shell's (every form but
/// `--store`, and the help forms).
pub fn parse_doctor(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) || !words.contains(&"--store") {
        return None;
    }
    const HELP: &str = concat!(" (", cli_name!(), " app doctor --help)");
    let (mut folder, mut voice, mut json) = (String::new(), "", false);
    for w in words {
        match *w {
            "--json" => json = true,
            "--store" => {}
            "--voice" => voice = "1",
            "--no-voice" => voice = "0",
            _ if w.starts_with('-') => {
                return Some(Err(Stop::usage(format!("Unknown flag: {w}{HELP}"))))
            }
            _ if folder.is_empty() => folder = w.to_string(),
            _ => return Some(Err(Stop::usage(format!("Unexpected argument: {w}{HELP}")))),
        }
    }
    if folder.is_empty() {
        folder = ".".into();
    }
    if !std::path::Path::new(&folder).is_dir() {
        return Some(Err(Stop::usage(format!(
            concat!(cli_name!(), " app doctor --store: {} is not a folder"),
            folder
        ))));
    }
    let folder = abs(&folder)?;
    Some(Ok(Ask::Store {
        folder,
        voice,
        json,
    }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Init {
            folder,
            template,
            name,
            app,
            voice,
            dry,
            json,
        } => {
            let bit = |b: bool| if b { "1" } else { "" };
            let b = Cj::new()
                .s("folder", folder)
                .s("template", template)
                .s("name", name)
                .s("app", app)
                .b("voice", bit(*voice))
                .b("dryRun", bit(*dry))
                .body();
            let body = post_200(ctx, "/cli/app/init", &b)?;
            Ok(super::access::answer(&body, *json, init_lines))
        }
        Ask::Store {
            folder,
            voice,
            json,
        } => {
            let b = Cj::new().s("folder", folder).b("voice", voice).body();
            let body = post_200(ctx, "/cli/app/doctor/store", &b)?;
            let mut shown = if *json {
                raw(&body)
            } else {
                let mut o = Out::default();
                let r = store_lines(&body, &mut o);
                let shown = o.done(r);
                if shown.exit != Exit::Ok {
                    return Ok(shown);
                }
                shown
            };
            // `python3 -c '…sys.exit(0 if json.load(sys.stdin).get("ok") else 1)' || exit 1`.
            let ok = (|| {
                let v = py::load(&body, 1)?;
                Ok::<bool, Raise>(py::truthy(&py::dget(&v, "ok", 1)?))
            })();
            match ok {
                Ok(true) => {}
                Ok(false) => shown.exit = Exit::Error,
                Err(e) => {
                    let Shown { stdout, stderr, .. } = shown;
                    shown = crate::families::raised(stdout, stderr, &e);
                }
            }
            Ok(shown)
        }
    }
}

/// `_app_rt_post`, then `[ "$_CP_STATUS" = "200" ] || _compute_fail`.
fn post_200(ctx: &mut Ctx, path: &str, body: &str) -> Result<Vec<u8>, Exit> {
    let s = ctx.send(true, path, body, false)?;
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

/// `cmd_app_init`'s Python.
fn init_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let dry = py::truthy(&py::dget(&v, "dryRun", 4)?);
    let words = [
        ("write", if dry { "would write" } else { "wrote" }),
        ("edit", if dry { "would edit" } else { "edited" }),
        ("skipped", "skipped"),
        ("snippet", "add by hand"),
    ];
    let files = py::or(py::dget(&v, "files", 6)?, py::empty_list());
    for f in py_iter(&files, 6)? {
        let action = py::dget(&f, "action", 7)?;
        let word = py::word_for(&words, &action, py::dget(&f, "action", 7)?, 7)?;
        o.line(&format!(
            "{} {}  ({})",
            py::left(&word, 12),
            py::s(&py::dget(&f, "path", 7)?),
            py::s(&py::dget(&f, "note", 7)?)
        ));
        if py::truthy(&py::dget(&f, "content", 8)?)
            && (dry || py_eq(&py::dget(&f, "action", 8)?, &py::text("snippet")))
        {
            let content = py::idx(&f, "content", 9)?;
            let Value::Str(t) = &content else {
                return Err(Raise::no_attribute(9, &content, "rstrip"));
            };
            let text = py::s(&Value::Str(t.clone()));
            for line in text.trim_end_matches('\n').split('\n') {
                o.line(&format!("    | {line}"));
            }
        }
    }
    let notes = py::or(py::dget(&v, "notes", 11)?, py::empty_list());
    for n in py_iter(&notes, 11)? {
        o.line(&py::concat("note: ", &n, 12)?);
    }
    if dry {
        o.line("Nothing was written (--dry-run).");
    }
    let next = py::or(py::dget(&v, "next", 15)?, py::empty_list());
    if py::truthy(&next) {
        o.line("Next:");
        for (i, c) in py_iter(&next, 18)?.iter().enumerate() {
            o.line(&format!("  {}. {}", i + 1, py::s(c)));
        }
    }
    Ok(Exit::Ok)
}

/// `_app_store_doctor`'s Python.
fn store_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    o.line(&format!(
        "Store check: {}",
        py::s(&py::dget(&v, "folder", 4)?)
    ));
    let checked = if py::truthy(&py::dget(&v, "voice", 5)?) {
        "checked"
    } else {
        "not checked"
    };
    o.line(&format!(
        "  calls: {checked} ({})",
        py::s(&py::dget(&v, "voiceWhy", 5)?)
    ));
    for k in ["tauri", "tao", "wry"] {
        if py::truthy(&py::dget(&v, k, 7)?) {
            o.line(&format!("  {k} {}", py::s(&py::idx(&v, k, 8)?)));
        }
    }
    let fs = py::or(py::dget(&v, "findings", 9)?, py::empty_list());
    let items = py_iter(&fs, 10)?;
    for f in &items {
        let level = py::idx(f, "level", 11)?;
        let Value::Str(t) = &level else {
            return Err(Raise::no_attribute(11, &level, "upper"));
        };
        let upper = py::s(&Value::Str(t.clone())).to_uppercase();
        o.line(&format!(
            "{} [{}] {}: {}",
            super::super::ljust(&upper, 5),
            py::s(&py::idx(f, "platform", 11)?),
            py::s(&py::idx(f, "path", 11)?),
            py::s(&py::idx(f, "problem", 11)?)
        ));
        o.line(&format!("      fix: {}", py::s(&py::idx(f, "fix", 12)?)));
    }
    let notes = py::or(py::dget(&v, "notes", 13)?, py::empty_list());
    for n in py_iter(&notes, 13)? {
        o.line(&py::concat("note: ", &n, 14)?);
    }
    let mut errs = 0usize;
    for f in &items {
        if py_eq(&py::idx(f, "level", 15)?, &py::text("error")) {
            errs += 1;
        }
    }
    let warns = py_len(&fs, 16)? as i64 - errs as i64;
    let passed = py::or(py::dget(&v, "passed", 17)?, py::empty_list());
    let s = |n: i64| if n == 1 { "" } else { "s" };
    if py::truthy(&py::dget(&v, "ok", 18)?) {
        let n = py_len(&passed, 19)? as i64;
        o.line(&format!(
            "Ready: no errors ({warns} warning{}), {n} check{} passed.",
            s(warns),
            s(n)
        ));
    } else {
        let e = errs as i64;
        o.line(&format!(
            "Not ready: {e} error{}, {warns} warning{}.",
            s(e),
            s(warns)
        ));
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_init_words_as_the_shell_reads_them() {
        let usage = |w: &[&str]| match parse_init(w) {
            Some(Err(s)) => s.line,
            other => panic!("{other:?}"),
        };
        assert!(usage(&[]).contains("--template is required: tauri, static or node-api ("));
        assert!(usage(&["--template", "x"]).ends_with("not 'x'"));
        assert!(usage(&["--template", "static", "--voice"]).ends_with("not --template static"));
        assert!(usage(&["--template=tauri", "--app", "a", "--name", "n"]).contains("not both"));
        assert!(
            usage(&["--template", "tauri", "/no/such/folder/here"]).ends_with("is not a folder")
        );
        assert_eq!(
            parse_init(&["--template"]),
            Some(Err(Stop::need_value("--template")))
        );
        assert_eq!(parse_init(&["--template", "tauri", "-h"]), None);
    }

    #[test]
    fn doctor_is_ours_only_with_store() {
        assert_eq!(parse_doctor(&[]), None);
        assert_eq!(parse_doctor(&["x"]), None);
        assert!(matches!(
            parse_doctor(&["--store", "a", "b"]),
            Some(Err(Stop {
                exit: Exit::Usage,
                ..
            }))
        ));
    }
}
