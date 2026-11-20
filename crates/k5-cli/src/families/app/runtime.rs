//! The App runtime's own checks, as the shell CLI answers them:
//! `app validate` (`cli/src/talk-and-workspace.sh`, `cli/src/app-doctor.sh`),
//! `app status` (`cli/src/send-feedback.sh`) and `app sdk add|status|kit`
//! (`cli/src/app-sdk.sh`).
//!
//! - `validate`: a folder with `manifest.json` (or `--manifest`), or a
//!   Seed hash, is `POST /cli/app/validate` (`_app_validate_seed`); when
//!   Apps are off a folder falls back to the code pass. A plain folder, an
//!   App name or nothing is the code pass, `GET /cli/publish/validate`
//!   over TCP (`cli_request`), printed by `_validate_print`.
//! - `status`: one `_compute_call`, the verdict read first (a raise there
//!   ends the command), `--json` as it came, else the shell's Python; the
//!   exit is the verdict's.
//! - `sdk`: the folder or file checked here, then one request; `--json`
//!   as it came, else the shell's Python.
//!
//! Where `_app_validate_seed` runs, the shell's `set -e` is off (it is the
//! left side of `&&`): its report printer may raise and the ok check still
//! runs, so a broken answer prints two tracebacks, as here.

use super::call::{Cj, Ctx, Split};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::sim::{basename, dirname};
use crate::families::{ljust, py_len, Raise, Shown};
use crate::out;
use crate::out::json::{self, Options, Value};
use crate::out::pylib;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Error};
use k2_client::{cli_name, product_name};

/// `app sdk`'s page (`_app_sdk_usage`).
pub const SDK_PAGE: &str = "app/sdk";

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const VALIDATE_FLAG_HEAD: &str = "Error: unknown flag '";
    pub const VALIDATE_FLAG_TAIL: &str = concat!(
        "' \u{2014} usage: ",
        cli_name!(),
        " app validate [<folder|seed|name>] [--dir <path>] [--manifest <file>] [--types] [--json]"
    );
    pub const BOTH: &str = "Error: give a folder, Seed or App name, or --dir, not both";
    pub const MANIFEST_WITH_FOLDER: &str = concat!(
        "Error: --manifest goes with a folder (",
        cli_name!(),
        " app validate <folder> --manifest <file>)"
    );
    pub const TYPES_FOLDER: &str =
        "Error: --types needs a folder with manifest.json (or a Seed) and Apps on";
    pub const TYPES_SEED: &str = "Error: --types needs a folder with manifest.json or a Seed";
    pub const STATUS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app status <app-id> [--json] [--no-checks] [--public]"
    );
    pub const STATUS_PUBLIC: &str = concat!(
        cli_name!(),
        " app status: --public runs the checks; it can't go with --no-checks"
    );
    pub const SDK: &str = concat!(
        "usage: ",
        cli_name!(),
        " app sdk add|status|kit \u{2026} (",
        cli_name!(),
        " app sdk --help)"
    );
    pub const SDK_FLAG_TAIL: &str = concat!(" (", cli_name!(), " app sdk --help)");
    pub const SDK_ADD: &str = concat!(
        "usage: ",
        cli_name!(),
        " app sdk add <app-folder> [--sew] [--json]"
    );
    pub const SDK_STATUS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app sdk status <app-folder> [--json]"
    );
    pub const SDK_KIT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app sdk kit <sew-app-kit-<version>.tar.gz> [--json]"
    );
    pub const SDK_HEAD: &str = concat!(cli_name!(), " app sdk ");
}

/// `cmd_app_validate`'s note when Apps are off and a folder falls back.
const APPS_OFF: &str = "note: Apps are off on this server (Settings \u{2192} Cores \u{2192} Apps \u{2192} This server), so manifest.json and the contract were not checked; code checks only";

/// `_validate_print`'s line for an empty answer.
const NO_RESPONSE: &str = concat!(
    "Error: no response from the ",
    product_name!(),
    " daemon on this computer"
);

/// `cmd_app_status`'s line for an answer with no verdict.
const NO_VERDICT: &str = concat!(cli_name!(), " app status: the daemon gave no verdict");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Validate(Validate),
    Status {
        query: String,
        json: bool,
    },
    /// `app sdk`, `app sdk help`: the page on stdout.
    SdkPage,
    /// `sdk add`: the body.
    SdkAdd {
        body: String,
        json: bool,
    },
    /// `sdk status`: the query.
    SdkStatus {
        query: String,
        json: bool,
    },
    /// `sdk kit`: the body.
    SdkKit {
        body: String,
        json: bool,
    },
}

/// `cmd_app_validate`'s words, after the checks that need no
/// `$PROJECT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validate {
    /// A folder (`--dir`, under `$PROJECT` when relative, or a path given
    /// as the target, made absolute).
    pub dir: String,
    /// `--manifest`, made absolute.
    pub manifest: String,
    /// A Seed hash, or an App name, when there is no folder.
    pub target: String,
    pub types: bool,
    pub json: bool,
}

/// The verbs here.
pub const VERBS: &[&str] = &["validate", "status", "sdk"];

/// The words after the verb. `None`: a help form (the front's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    Some(match verb {
        "validate" => validate(words).map(Ask::Validate),
        "status" => status(words),
        _ => sdk(words),
    })
}

fn is_dir(p: &str) -> bool {
    !p.is_empty() && std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

fn is_file(p: &str) -> bool {
    !p.is_empty() && std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// `cd <p> && pwd -P`.
fn physical(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `_app_rt_abs`: a folder's physical path, anything else as it is.
pub(super) fn rt_abs(p: &str) -> String {
    if is_dir(p) {
        physical(p)
    } else {
        p.to_string()
    }
}

/// `_app_rt_abs_file`: the file's folder resolved, when it is one.
pub(super) fn rt_abs_file(p: &str) -> String {
    let d = dirname(p);
    if is_dir(&d) {
        format!("{}/{}", physical(&d), basename(p))
    } else {
        p.to_string()
    }
}

/// `[[ "$t" =~ ^[0-9a-f]{12,64}$ ]]`.
fn seed_hash(t: &str) -> bool {
    (12..=64).contains(&t.len()) && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn validate(words: &[&str]) -> Result<Validate, Stop> {
    let (mut dir, mut target, mut manifest) = (String::new(), String::new(), String::new());
    let (mut types, mut json) = (false, false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            // `x="${2-}"; [ $# -ge 2 ] && shift 2 || shift`.
            "--dir" | "--manifest" => {
                let v = words.get(i + 1).copied().unwrap_or("").to_string();
                if w == "--dir" {
                    dir = v;
                } else {
                    manifest = v;
                }
                i += 1;
            }
            "--types" => types = true,
            "--json" => json = true,
            _ if w.starts_with("--dir=") => dir = w["--dir=".len()..].to_string(),
            _ if w.starts_with("--manifest=") => manifest = w["--manifest=".len()..].to_string(),
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "{}{w}{}",
                    usage::VALIDATE_FLAG_HEAD,
                    usage::VALIDATE_FLAG_TAIL
                )))
            }
            _ if target.is_empty() => target = w.to_string(),
            _ => return Err(Stop::usage(format!("Error: unexpected argument '{w}'"))),
        }
        i += 1;
    }
    if !target.is_empty() && !dir.is_empty() {
        return Err(Stop::usage(usage::BOTH));
    }
    if !target.is_empty() && is_dir(&target) {
        dir = rt_abs(&target);
        target = String::new();
    }
    if !manifest.is_empty() {
        if !is_file(&manifest) {
            return Err(Stop::usage(format!(
                "Error: --manifest {manifest} is not a file"
            )));
        }
        if dir.is_empty() {
            return Err(Stop::usage(usage::MANIFEST_WITH_FOLDER));
        }
        manifest = rt_abs_file(&manifest);
    }
    if dir.is_empty() && types && !seed_hash(&target) {
        return Err(Stop::usage(usage::TYPES_SEED));
    }
    Ok(Validate {
        dir,
        manifest,
        target,
        types,
        json,
    })
}

fn status(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json, mut nochecks, mut public) = (String::new(), false, false, false);
    for w in words {
        match *w {
            "--json" => json = true,
            "--no-checks" => nochecks = true,
            "--public" => public = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!("Unknown flag: {w}\n{}", usage::STATUS)))
            }
            _ if !app.is_empty() => return Err(Stop::usage(usage::STATUS)),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::STATUS));
    }
    if nochecks && public {
        return Err(Stop::usage(usage::STATUS_PUBLIC));
    }
    let mut query = format!("app={}", quote(&app));
    if nochecks {
        query.push_str("&noChecks=1");
    }
    if public {
        query.push_str("&public=1");
    }
    Ok(Ask::Status { query, json })
}

/// One `sdk` verb's loop: `--json`, `--sew` where `sew` allows it, and
/// one word. The word, `--sew` and `--json`.
fn sdk_words(words: &[&str], sew_ok: bool, line: &str) -> Result<(String, bool, bool), Stop> {
    let (mut word, mut sew, mut json) = (String::new(), false, false);
    for w in words {
        match *w {
            "--sew" if sew_ok => sew = true,
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::SDK_FLAG_TAIL
                )))
            }
            _ if !word.is_empty() => return Err(Stop::usage(line)),
            _ => word = w.to_string(),
        }
    }
    if word.is_empty() {
        return Err(Stop::usage(line));
    }
    Ok((word, sew, json))
}

fn sdk(words: &[&str]) -> Result<Ask, Stop> {
    let (sub, rest) = match words.split_first() {
        Some((s, r)) => (*s, r),
        None => ("", words),
    };
    match sub {
        "add" => {
            let (folder, sew, json) = sdk_words(rest, true, usage::SDK_ADD)?;
            if !is_dir(&folder) {
                return Err(Stop::usage(format!(
                    "{}add: {folder} is not a folder",
                    usage::SDK_HEAD
                )));
            }
            let body = Cj::new()
                .s("folder", &rt_abs(&folder))
                .b("sew", if sew { "1" } else { "0" })
                .body();
            Ok(Ask::SdkAdd { body, json })
        }
        "status" => {
            let (folder, _, json) = sdk_words(rest, false, usage::SDK_STATUS)?;
            if !is_dir(&folder) {
                return Err(Stop::usage(format!(
                    "{}status: {folder} is not a folder",
                    usage::SDK_HEAD
                )));
            }
            let query = format!("folder={}", quote(&rt_abs(&folder)));
            Ok(Ask::SdkStatus { query, json })
        }
        "kit" => {
            let (path, _, json) = sdk_words(rest, false, usage::SDK_KIT)?;
            if !is_file(&path) {
                return Err(Stop::usage(format!(
                    "{}kit: {path} is not a file",
                    usage::SDK_HEAD
                )));
            }
            let abs = format!("{}/{}", physical(&dirname(&path)), basename(&path));
            let body = Cj::new().s("path", &abs).body();
            Ok(Ask::SdkKit { body, json })
        }
        "" | "help" => Ok(Ask::SdkPage),
        _ => Err(Stop::usage(usage::SDK)),
    }
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Validate(v) => run_validate(ctx, v),
        Ask::Status { query, json } => {
            let body = ctx.call(false, "/cli/app/status", query)?;
            Ok(status_answer(&body, *json))
        }
        Ask::SdkPage => Ok(Shown::out(
            crate::text::help(SDK_PAGE).unwrap_or_default().to_string(),
        )),
        Ask::SdkAdd { body, json } => {
            let s = ctx.send(true, "/cli/app/sdk/add", body, false)?;
            let b = ok_body(ctx, s)?;
            Ok(answer(&b, *json, sdk_add_lines))
        }
        Ask::SdkStatus { query, json } => {
            let b = ctx.call(false, "/cli/app/sdk/status", query)?;
            Ok(answer(&b, *json, sdk_status_lines))
        }
        Ask::SdkKit { body, json } => {
            let s = ctx.send(true, "/cli/app/sdk/kit", body, false)?;
            let b = ok_body(ctx, s)?;
            Ok(answer(&b, *json, sdk_kit_lines))
        }
    }
}

/// `[ "$_CP_STATUS" = "200" ] || _compute_fail …`.
fn ok_body(ctx: &Ctx, s: Split) -> Result<Vec<u8>, Exit> {
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

fn answer(body: &[u8], json: bool, lines: fn(&[u8], &mut Out) -> Result<Exit, Raise>) -> Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = lines(body, &mut o);
    o.done(r)
}

/// What one Python step printed and how it ended, without ending the
/// command (the shell's `set -e` is off there).
fn step(o: &mut Out, r: Result<Exit, Raise>) -> Exit {
    match r {
        Ok(e) => e,
        Err(e) => {
            o.stderr.push_str(&e.text());
            Exit::Error
        }
    }
}

/// What `cmd_app_validate` sends first, with `$PROJECT` known: a
/// folder's or a Seed's `_app_validate_seed` body, or none (the code pass
/// only). `Err`: the `--types` line (exit 2).
fn seed_body(ctx: &Ctx, v: &Validate) -> Result<Option<String>, Exit> {
    let types_b = if v.types { "1" } else { "0" };
    if !v.dir.is_empty() {
        let abs = if v.dir.starts_with('/') {
            v.dir.clone()
        } else {
            let ws = ctx.project.strip_suffix('/').unwrap_or(&ctx.project);
            format!("{ws}/{}", v.dir)
        };
        if is_file(&format!("{abs}/manifest.json")) || !v.manifest.is_empty() {
            return Ok(Some(
                Cj::new()
                    .s("folder", &rt_abs(&abs))
                    .b("types", types_b)
                    .s("manifest", &v.manifest)
                    .body(),
            ));
        }
        if v.types {
            out::stderr_line(usage::TYPES_FOLDER);
            return Err(Exit::Usage);
        }
        return Ok(None);
    }
    if seed_hash(&v.target) {
        return Ok(Some(
            Cj::new().s("seed", &v.target).b("types", types_b).body(),
        ));
    }
    Ok(None)
}

fn run_validate(ctx: &mut Ctx, v: &Validate) -> Answer {
    if let Some(body) = seed_body(ctx, v)? {
        let s = ctx.send(true, "/cli/app/validate", &body, false)?;
        let apps_off = s.status == "404"
            && s.body
                .windows(b"\"apps_disabled\"".len())
                .any(|w| w == b"\"apps_disabled\"");
        if !apps_off {
            if !s.ok() {
                return Err(ctx.fail(&s));
            }
            let mut o = Out::default();
            if v.json {
                o.stdout
                    .push_str(&format!("{}\n", String::from_utf8_lossy(&s.body)));
            } else {
                let r = report_lines(&s.body, &mut o);
                step(&mut o, r);
            }
            let r = ok_check(&s.body);
            let ok = step(&mut o, r);
            return Ok(Shown {
                stdout: o.stdout,
                stderr: o.stderr,
                exit: if ok == Exit::Ok {
                    Exit::Ok
                } else {
                    Exit::Error
                },
            });
        }
        if v.dir.is_empty() {
            // The Seed form: Apps off is `_compute_fail "404" …`.
            return Err(ctx.fail(&s));
        }
        out::stderr_line(APPS_OFF);
        if v.types {
            out::stderr_line(usage::TYPES_FOLDER);
            return Err(Exit::Usage);
        }
    }
    let name = if v.dir.is_empty() {
        v.target.as_str()
    } else {
        ""
    };
    code_pass(ctx, name, &v.dir, v.json)
}

/// `_app_validate_code <name> <dir> <json>`: the code pass over TCP, and
/// `_validate_print`.
fn code_pass(ctx: &mut Ctx, name: &str, dir: &str, json: bool) -> Answer {
    let mut dir = dir.to_string();
    if dir.starts_with('/') {
        let ws = ctx.project.strip_suffix('/').unwrap_or(&ctx.project);
        if dir == ws {
            dir = ".".to_string();
        } else if let Some(rest) = dir.strip_prefix(&format!("{ws}/")) {
            dir = rest.to_string();
        }
    }
    let mut q = Vec::new();
    if !name.is_empty() {
        q.push(format!("name={}", quote(name)));
    }
    if !dir.is_empty() {
        q.push(format!("dir={}", quote(&dir)));
    }
    let pairs: Vec<&str> = q.iter().map(String::as_str).collect();
    let passport = ctx.client.passport().is_some();
    let raw = match ctx
        .client
        .call(Channel::Tcp, "/cli/publish/validate", Call::Request(&pairs))
    {
        Ok(r) => {
            ctx.closing.answer(&r, passport);
            crate::families::users::held(&r.body)
        }
        Err(
            e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. }),
        ) => {
            out::stderr_line(&e.to_string());
            return Err(Exit::Error);
        }
        Err(_) => Vec::new(),
    };
    let what = if !name.is_empty() {
        name.to_string()
    } else if !dir.is_empty() {
        dir
    } else {
        "this workspace".to_string()
    };
    let mut o = Out::default();
    let r = validate_print(&raw, json, &what, &mut o);
    Ok(o.done(r))
}

/// `_validate_print`'s Python (`V_RAW`, `V_JSON`, `V_WHAT`).
pub fn validate_print(raw: &[u8], as_json: bool, what: &str, o: &mut Out) -> Result<Exit, Raise> {
    let raw = String::from_utf8_lossy(raw).into_owned();
    if raw.trim_matches(crate::families::py_space).is_empty() {
        o.err(NO_RESPONSE);
        return Ok(Exit::Error);
    }
    let head: String = raw.chars().take(200).collect();
    let d = match pylib::json::loads(&raw) {
        Ok(d) => d,
        Err(_) => {
            o.err(&format!(
                "Error: the daemon answered something that is not JSON: {head}"
            ));
            return Ok(Exit::Error);
        }
    };
    if !matches!(d, Value::Object(_)) {
        o.err(&format!("Error: unexpected answer: {head}"));
        return Ok(Exit::Error);
    }
    let err = py::dget(&d, "error", 16)?;
    if err != Value::Null {
        if as_json {
            o.line(&dumps(&d, true, 19)?);
        }
        let mut msg = py::dget(&d, "message", 20)?;
        if !py::truthy(&msg) {
            msg = match &err {
                Value::Object(_) => py::dget(&err, "message", 20)?,
                other => other.clone(),
            };
        }
        o.err(&format!("Error: {}", py::s(&msg)));
        if py::truthy(&py::dget(&d, "suggestion", 22)?) {
            o.err(&format!(
                "  suggestion: {}",
                py::s(&py::idx(&d, "suggestion", 23)?)
            ));
        }
        return Ok(Exit::Error);
    }
    let mut status = py::dget(&d, "status", 25)?;
    if !py::truthy(&status) {
        status = py::text(if py::truthy(&py::dget(&d, "ok", 25)?) {
            "ok"
        } else {
            "failed"
        });
    }
    let code = match &status {
        Value::Array(_) | Value::Object(_) => {
            return Err(Raise::at(
                26,
                format!(
                    "TypeError: unhashable type: '{}'",
                    crate::out::py::type_name(&status)
                ),
            ))
        }
        s if *s == py::text("ok") => Exit::Ok,
        s if *s == py::text("pending") => Exit::Usage,
        _ => Exit::Error,
    };
    if as_json {
        o.line(&dumps(&d, false, 28)?);
        return Ok(code);
    }
    let line = |e: &Value, warn: bool, from: u32| -> Result<String, Raise> {
        let inside = |r: Raise| Raise {
            frames: vec![(from, "<module>"), (r.frames[0].0, "line")],
            exception: r.exception,
        };
        let file = py::dget(e, "file", 31).map_err(inside)?;
        let ln = py::dget(e, "line", 31).map_err(inside)?;
        let col = py::dget(e, "col", 31).map_err(inside)?;
        let message = py::dget(e, "message", 31).map_err(inside)?;
        let mut s = format!(
            "{}:{}:{}: {}{}",
            py::s(&file),
            py::s(&ln),
            py::s(&col),
            if warn { "warning: " } else { "" },
            py::s(&message)
        );
        if py::truthy(&py::dget(e, "suggestion", 32).map_err(inside)?) {
            let sug = py::idx(e, "suggestion", 33).map_err(inside)?;
            s.push_str(&format!("\n  suggestion: {}", py::s(&sug)));
        }
        Ok(s)
    };
    let errors = py::or(py::dget(&d, "errors", 35)?, py::empty_list());
    for e in py::iter(&errors, 35)? {
        o.line(&line(&e, false, 36)?);
    }
    let warnings = py::or(py::dget(&d, "warnings", 37)?, py::empty_list());
    for w in py::iter(&warnings, 37)? {
        o.line(&line(&w, true, 38)?);
    }
    let skipped = py::or(py::dget(&d, "skipped", 39)?, py::empty_list());
    for s in py::iter(&skipped, 39)? {
        o.line(&format!("skipped: {}", py::s(&s)));
    }
    let errs = py_len(&py::or(py::dget(&d, "errors", 41)?, py::empty_list()), 41)?;
    let warns = py_len(&py::or(py::dget(&d, "warnings", 42)?, py::empty_list()), 42)?;
    let target = py::or(py::dget(&d, "target", 45)?, py::text(what));
    let files = py::dget(&d, "files", 45)?;
    if errs > 0 {
        o.err(&format!(
            "{errs} error(s), {warns} warning(s) in {} file(s) of {}",
            py::s(&files),
            py::s(&target)
        ));
    } else {
        let tail = if warns > 0 {
            format!("; {warns} warning(s)")
        } else {
            String::new()
        };
        o.line(&format!(
            "ok: {} file(s) of {} parse{tail}",
            py::s(&files),
            py::s(&target)
        ));
    }
    Ok(code)
}

/// `json.dumps(d, separators=(",", ":"))` (`compact`) or
/// `json.dumps(d, indent=2)`.
fn dumps(d: &Value, compact: bool, line: u32) -> Result<String, Raise> {
    let options = Options {
        compact,
        indent: if compact { None } else { Some(2) },
        ..Options::default()
    };
    json::to_string(d, &options).map_err(|e| Raise::at(line, format!("ValueError: {e:?}")))
}

/// `_app_rt_report`'s Python.
pub fn report_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let mut r = py::dget(&v, "report", 4)?;
    if !py::truthy(&r) {
        let e = py::or(py::dget(&v, "error", 4)?, Value::Object(Vec::new()));
        r = py::or(py::dget(&e, "report", 4)?, Value::Object(Vec::new()));
    }
    for (word, key, line) in [("error  ", "errors", 5), ("warning", "warnings", 8)] {
        let list = py::dget_or(&r, key, py::empty_list(), line)?;
        for e in py::iter(&list, line)? {
            let at = if py::truthy(&py::dget(&e, "at", line + 1)?) {
                let at = py::idx(&e, "at", line + 1)?;
                format!("{})", py::concat(" (", &at, line + 1)?)
            } else {
                String::new()
            };
            let code = py::dget_or(&e, "code", py::text(""), line + 2)?;
            let msg = py::dget_or(&e, "message", py::text(""), line + 2)?;
            o.line(&format!("{word} {}{at}: {}", py::s(&code), py::s(&msg)));
        }
    }
    let checks = py::join(", ", &py::dget_or(&r, "checks", py::empty_list(), 11)?, 11)?;
    let ok = if py::truthy(&py::dget(&r, "ok", 12)?) {
        "ok"
    } else {
        "FAILED"
    };
    let tail = if checks.is_empty() {
        String::new()
    } else {
        format!(" \u{2014} checks: {checks}")
    };
    o.line(&format!("{ok}{tail}"));
    Ok(Exit::Ok)
}

/// `sys.exit(0 if json.load(sys.stdin).get("ok") else 1)`.
fn ok_check(body: &[u8]) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    Ok(if py::truthy(&py::dget(&v, "ok", 1)?) {
        Exit::Ok
    } else {
        Exit::Error
    })
}

/// `print(json.load(sys.stdin).get("verdict",""))` as `$(…)` holds it.
fn verdict(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 1)?;
    let mut t = py::s(&py::dget_or(&v, "verdict", py::text(""), 1)?);
    while t.ends_with('\n') {
        t.pop();
    }
    Ok(t)
}

fn status_answer(body: &[u8], json: bool) -> Shown {
    let verdict = match verdict(body) {
        Ok(v) => v,
        Err(e) => return crate::families::raised(String::new(), String::new(), &e),
    };
    let mut o = Out::default();
    if json {
        o.stdout
            .push_str(&format!("{}\n", String::from_utf8_lossy(body)));
    } else if let Err(e) = status_lines(body, &mut o) {
        return crate::families::raised(o.stdout, o.stderr, &e);
    }
    let exit = match verdict.as_str() {
        "green" => Exit::Ok,
        "yellow" => Exit::Usage,
        "red" => Exit::Error,
        _ => {
            o.err(NO_VERDICT);
            Exit::Error
        }
    };
    Shown {
        stdout: o.stdout,
        stderr: o.stderr,
        exit,
    }
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `x.get(key) or {}`.
fn dict_of(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    Ok(py::or(py::dget(v, key, line)?, empty_dict()))
}

/// `x.get(key) or "-"`.
fn dash(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    Ok(py::or(py::dget(v, key, line)?, py::text("-")))
}

/// `x.get("status") if x.get("status") is not None else "no answer"`.
fn got(v: &Value, line: u32) -> Result<Value, Raise> {
    let s = py::dget(v, "status", line)?;
    Ok(if s == Value::Null {
        py::text("no answer")
    } else {
        s
    })
}

/// `cmd_app_status`'s Python.
pub fn status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let d = dict_of(&v, "deploy", 4)?;
    let h = dict_of(&v, "health", 4)?;
    let c = dict_of(&v, "checks", 4)?;
    let app = py::idx(&v, "app", 5)?;
    let verdict = py::dget_or(&v, "verdict", py::text(""), 5)?;
    let upper = match &verdict {
        Value::Str(_) => py::s(&verdict).to_uppercase(),
        other => return Err(Raise::no_attribute(5, other, "upper")),
    };
    let state = py::dget_or(&d, "state", py::text(""), 5)?;
    o.line(&format!("{}  {upper}  ({})", py::s(&app), py::s(&state)));
    if py::truthy(&py::dget(&d, "stateReason", 6)?) {
        o.line(&format!(
            "  why        {}",
            py::s(&py::idx(&d, "stateReason", 7)?)
        ));
    }
    let seed = dict_of(&d, "seed", 8)?;
    let live = dict_of(&d, "live", 8)?;
    let last = dict_of(&d, "last", 8)?;
    if py::truthy(&seed) {
        let hash = py::head(&py::or(py::dget(&seed, "hash", 10)?, py::text("")), 12, 10)?;
        let version = if py::truthy(&py::dget(&seed, "version", 10)?) {
            py::concat(" v", &py::idx(&seed, "version", 10)?, 10)?
        } else {
            String::new()
        };
        let live_at = dash(&live, "liveAt", 11)?;
        let actor = dash(&live, "actor", 11)?;
        let commit = py::head(&dash(&live, "commit", 11)?, 12, 11)?;
        o.line(&format!(
            "  deploy     Seed {}{version}, live since {} by {}, commit {}",
            py::s(&hash),
            py::s(&live_at),
            py::s(&actor),
            py::s(&commit)
        ));
    } else {
        o.line("  deploy     nothing deployed yet");
    }
    if py::truthy(&last) {
        let err = if py::truthy(&py::dget(&last, "error", 15)?) {
            py::text(&py::concat(": ", &py::idx(&last, "error", 15)?, 15)?)
        } else {
            py::text("")
        };
        let kind = py::dget_or(&last, "kind", py::text(""), 16)?;
        let id = py::dget_or(&last, "id", py::text(""), 16)?;
        let result = py::dget_or(&last, "result", py::text(""), 16)?;
        o.line(&format!(
            "             last: {} {} {}{}",
            py::s(&kind),
            py::s(&id),
            py::s(&result),
            py::s(&py::head(&err, 160, 16)?)
        ));
    }
    let p = py::dget(&h, "probe", 17)?;
    if py::truthy(&p) {
        let st = got(&p, 19)?;
        o.line(&format!(
            "  health     GET {}: {} in {} ms; up {}s; restarts {}",
            py::s(&py::dget(&h, "path", 20)?),
            py::s(&st),
            py::s(&py::dget(&p, "ms", 20)?),
            py::s(&py::dget(&h, "uptimeSecs", 20)?),
            py::s(&py::dget(&h, "recentRestarts", 20)?)
        ));
    } else {
        o.line("  health     nothing live to probe");
    }
    if py::truthy(&py::dget(&c, "results", 23)?) {
        o.line(&format!(
            "  checks     {} of {} failed ({})",
            py::s(&py::dget_or(&c, "failed", Value::Int("0".into()), 24)?),
            py::s(&py::dget(&c, "declared", 24)?),
            py::s(&py::dget_or(&c, "source", py::text(""), 24)?)
        ));
        for r in py::iter(&py::idx(&c, "results", 25)?, 25)? {
            let g = got(&r, 26)?;
            let extra = if py::truthy(&py::dget(&r, "ok", 27)?) {
                String::new()
            } else {
                format!("  \u{2717} {}", py::s(&py::dget(&r, "why", 27)?))
            };
            let name = py::idx(&r, "name", 28)?;
            let path = py::idx(&r, "path", 28)?;
            let expect = py::idx(&r, "expect", 28)?;
            o.line(&format!(
                "             {} GET {}: expect {}, got {}{extra}",
                ljust(&py::s(&name), 16),
                py::s(&path),
                py::s(&expect),
                py::s(&g)
            ));
        }
    } else if py::truthy(&py::dget(&c, "recorded", 29)?) {
        let rec = py::or(
            py::dget(&py::idx(&c, "recorded", 30)?, "results", 30)?,
            py::empty_list(),
        );
        let failed = py::dget(&c, "failed", 31)?;
        let n = py_len(&rec, 31)?;
        o.line(&format!(
            "  checks     recorded at the switch: {} of {n} failed",
            py::s(&failed)
        ));
    } else if py::truthy(&py::dget(&c, "error", 32)?) {
        o.line(&format!(
            "  checks     {}",
            py::s(&py::idx(&c, "error", 33)?)
        ));
    } else {
        o.line("  checks     none declared (host.checks in the manifest)");
    }
    let e = dict_of(&v, "errors", 36)?;
    if py::contains(&e, "unavailable", 37)? {
        o.line(&format!(
            "  errors     unavailable ({})",
            py::s(&py::idx(&e, "unavailable", 38)?)
        ));
    } else {
        let by_kind = dict_of(&e, "byKind", 40)?;
        let Value::Object(pairs) = &by_kind else {
            return Err(Raise::no_attribute(40, &by_kind, "items"));
        };
        let parts: Vec<String> = pairs
            .iter()
            .map(|(k, n)| format!("{} {}", py::s(&Value::Str(k.clone())), py::s(n)))
            .collect();
        let by = if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        };
        o.line(&format!("  errors     last hour: {by}"));
    }
    let r = dict_of(&v, "resources", 42)?;
    if py::contains(&r, "unavailable", 43)? {
        o.line(&format!(
            "  resources  unavailable ({})",
            py::s(&py::idx(&r, "unavailable", 44)?)
        ));
    } else {
        let a = dict_of(&r, "app", 46)?;
        let disk = py::dget(&a, "diskBytes", 47)?;
        let shown = match &disk {
            Value::Int(_) | Value::Float(_) | Value::Bool(_) => {
                format!("{} MB", py::f(py::div(&disk, 1e6, 48)?, 0, 1))
            }
            _ => "not measured yet".to_string(),
        };
        o.line(&format!(
            "  resources  disk {shown}; box disk level {}",
            py::s(&py::dget(&r, "level", 48)?)
        ));
    }
    let dr = dict_of(&v, "door", 49)?;
    o.line(&format!(
        "  door       egress refused last hour: {}; waiting for the owner: {}",
        py::s(&py::dget(&dr, "egressDenied", 50)?),
        py::s(&py::dget(&dr, "heldApprovals", 50)?)
    ));
    let ho = dict_of(&v, "hosts", 51)?;
    let doms = dict_of(&ho, "domains", 52)?;
    let nd = if matches!(doms, Value::Object(_)) {
        py_len(
            &py::or(py::dget(&doms, "domains", 53)?, py::empty_list()),
            53,
        )?
    } else {
        0
    };
    let host = py::or(
        py::dget(&ho, "url", 54)?,
        py::or(py::dget(&ho, "label", 54)?, py::text("-")),
    );
    let tail = if nd > 0 {
        format!("; {nd} custom domain(s)")
    } else {
        String::new()
    };
    o.line(&format!("  hosts      {}{tail}", py::s(&host)));
    let ph = dict_of(&v, "phone", 55)?;
    let pu = dict_of(&ph, "push", 56)?;
    let pa = dict_of(&ph, "pairing", 56)?;
    let keys = dict_of(&pu, "keys", 57)?;
    let on = |v: &Value| if py::truthy(v) { "on" } else { "off" };
    let set = |v: &Value| if py::truthy(v) { "set" } else { "not set" };
    let pairing = on(&py::dget(&pa, "on", 58)?);
    let push = on(&py::dget(&pu, "on", 58)?);
    let apns = set(&py::dget(&keys, "apns", 59)?);
    let fcm = set(&py::dget(&keys, "fcm", 59)?);
    let devices = py::dget_or(&pu, "devices", Value::Int("0".into()), 59)?;
    o.line(&format!(
        "  phone      pairing {pairing}; push {push} (APNs key {apns}, FCM key {fcm}, {} device(s))",
        py::s(&devices)
    ));
    let u = dict_of(&v, "updates", 60)?;
    o.line(&format!(
        "  updates    feed {}",
        py::s(&dash(&u, "feed", 61)?)
    ));
    let nx = py::or(py::dget(&v, "next", 62)?, py::empty_list());
    if py::truthy(&nx) {
        o.line("next");
        for n in py::iter(&nx, 65)? {
            let level = py::idx(&n, "level", 66)?;
            let what = py::idx(&n, "what", 66)?;
            let run = py::idx(&n, "run", 66)?;
            o.line(&format!(
                "  [{}] {}: {}",
                py::s(&level),
                py::s(&what),
                py::s(&run)
            ));
        }
    }
    Ok(Exit::Ok)
}

/// `len(v)` for the width sum: a `TypeError` for a value with none.
fn len_of(v: &Value, line: u32) -> Result<usize, Raise> {
    py_len(v, line)
}

/// `_app_sdk_add`'s Python.
pub fn sdk_add_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let folder = py::idx(&v, "folder", 4)?;
    o.line(&format!("Vendored into {}/vendor:", py::s(&folder)));
    let vendored = py::idx(&v, "vendored", 5)?;
    let items = py::iter(&vendored, 5)?;
    let mut w: Option<usize> = None;
    for x in &items {
        let n = len_of(&py::idx(x, "name", 5)?, 5)? + len_of(&py::idx(x, "version", 5)?, 5)?;
        w = Some(w.map_or(n, |m| m.max(n)));
    }
    let Some(w) = w.map(|m| m + 2) else {
        return Err(Raise::at(5, "ValueError: max() arg is an empty sequence"));
    };
    for x in py::iter(&py::idx(&v, "vendored", 6)?, 6)? {
        let name = py::concat("", &py::idx(&x, "name", 7)?, 7)?;
        let label = py::concat(&format!("{name} "), &py::idx(&x, "version", 7)?, 7)?;
        let file = py::idx(&x, "file", 7)?;
        let tail = if py::truthy(&py::idx(&x, "written", 7)?) {
            ""
        } else {
            "  (already there)"
        };
        o.line(&format!("  {} {}{tail}", ljust(&label, w), py::s(&file)));
    }
    let removed = py::or(py::dget(&v, "removed", 8)?, py::empty_list());
    for r in py::iter(&removed, 8)? {
        o.line(&format!("  removed the older {}", py::s(&r)));
    }
    let mut names = Vec::new();
    for x in py::iter(&py::idx(&v, "vendored", 10)?, 10)? {
        names.push(py::idx(&x, "name", 10)?);
    }
    let deps = py::join(", ", &Value::Array(names), 10)?;
    let pj = py::idx(&v, "packageJson", 11)?;
    let mut line = format!("package.json: {} (dependencies: {deps}", py::s(&pj));
    if py::truthy(&py::dget(&v, "overrides", 12)?) {
        line.push_str("; overrides: ");
        line.push_str(&py::join(", ", &py::idx(&v, "overrides", 13)?, 13)?);
    }
    o.line(&format!("{line})"));
    o.line(&format!("Next, in {}:", py::s(&py::idx(&v, "folder", 15)?)));
    if py::truthy(&py::dget(&v, "install", 16)?) {
        o.line(&py::concat("  ", &py::idx(&v, "install", 17)?, 17)?);
        o.line("  (the peers and npm-registry dependencies of the Sew packages; it also installs the vendored ones)");
    } else {
        o.line("  npm install");
    }
    // The shell's own quotes end its single-quoted Python here, so the
    // package's name prints with none (a bash bug, copied).
    o.line("  import { fromEnv } from @k2/server");
    o.line("Keep vendor/ with the App (not in .k2appignore): its Seed carries it.");
    Ok(Exit::Ok)
}

/// `"none (Sew %s has no App kit; 0.5.0+)"`.
const NO_KIT: &str = concat!("none (", cli_name!(), " app sdk kit)");
/// `"Update: k2 app sdk add %s%s"`.
const UPDATE: &str = concat!("Update: ", cli_name!(), " app sdk add ");

/// `_app_sdk_status`'s Python.
pub fn sdk_status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let b = py::idx(&v, "box", 4)?;
    let server = py::idx(&b, "k2Server", 5)?;
    let mut kit = py::dget(&b, "sewKit", 5)?;
    if !py::truthy(&kit) {
        kit = if py::truthy(&py::dget(&b, "sewKitPinned", 5)?) {
            py::text(NO_KIT)
        } else {
            py::text(&format!(
                "none (Sew {} has no App kit; 0.5.0+)",
                py::s(&py::dget(&b, "sewPin", 5)?)
            ))
        };
    }
    o.line(&format!(
        "This box: @k2/server {}; Sew App kit {}",
        py::s(&server),
        py::s(&kit)
    ));
    for r in py::iter(&py::idx(&v, "packages", 6)?, 6)? {
        let name = py::idx(&r, "name", 7)?;
        let vend = dash(&r, "vendored", 7)?;
        let on_box = dash(&r, "onBox", 7)?;
        let state = py::idx(&r, "state", 7)?;
        o.line(&format!(
            "  {} vendored {} box {} {}",
            ljust(&py::s(&name), 14),
            ljust(&py::s(&vend), 10),
            ljust(&py::s(&on_box), 10),
            py::s(&state)
        ));
    }
    let mut stale = false;
    for r in py::iter(&py::idx(&v, "packages", 8)?, 8)? {
        let st = py::idx(&r, "state", 8)?;
        if st == py::text("older") || st == py::text("missing") {
            stale = true;
            break;
        }
    }
    if stale {
        let folder = py::idx(&v, "folder", 9)?;
        let mut sew = false;
        for r in py::iter(&py::idx(&v, "packages", 9)?, 9)? {
            let name = py::idx(&r, "name", 9)?;
            let Value::Str(_) = &name else {
                return Err(Raise::no_attribute(9, &name, "startswith"));
            };
            if py::s(&name).starts_with("@sew/") {
                sew = true;
                break;
            }
        }
        o.line(&format!(
            "{UPDATE}{}{}",
            py::s(&folder),
            if sew { " --sew" } else { "" }
        ));
    }
    Ok(Exit::Ok)
}

/// `"Now: k2 app sdk add <app-folder> --sew"`.
const NOW_ADD: &str = concat!("Now: ", cli_name!(), " app sdk add <app-folder> --sew");

/// `_app_sdk_kit`'s Python.
pub fn sdk_kit_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let version = py::idx(&v, "version", 4)?;
    let sha = py::head(&py::idx(&v, "sha256", 4)?, 12, 4)?;
    let mut names = Vec::new();
    for p in py::iter(&py::idx(&v, "packages", 4)?, 4)? {
        names.push(py::idx(&p, "name", 4)?);
    }
    let names = py::join(", ", &Value::Array(names), 4)?;
    o.line(&format!(
        "Kept Sew App kit {} (sha256 {}): {names}",
        py::s(&version),
        py::s(&sha)
    ));
    o.line(NOW_ADD);
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_hash_is_twelve_to_sixty_four_lower_hex() {
        assert!(seed_hash("0123456789ab"));
        assert!(!seed_hash("0123456789a"));
        assert!(!seed_hash("0123456789AB"));
        assert!(seed_hash(&"f".repeat(64)));
        assert!(!seed_hash(&"f".repeat(65)));
    }

    #[test]
    fn status_words_stop_as_the_shell_does() {
        assert_eq!(status(&[]), Err(Stop::usage(usage::STATUS)));
        assert_eq!(
            status(&["a", "--no-checks", "--public"]),
            Err(Stop::usage(usage::STATUS_PUBLIC))
        );
        assert_eq!(
            status(&["a b", "--public"]),
            Ok(Ask::Status {
                query: "app=a%20b&public=1".into(),
                json: false
            })
        );
    }

    #[test]
    fn sdk_alone_is_its_page_and_an_unknown_word_its_usage() {
        assert_eq!(sdk(&[]), Ok(Ask::SdkPage));
        assert_eq!(sdk(&["help"]), Ok(Ask::SdkPage));
        assert_eq!(sdk(&["nope"]), Err(Stop::usage(usage::SDK)));
    }
}
