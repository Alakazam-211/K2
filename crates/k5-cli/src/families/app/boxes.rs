//! The box the Apps run on, as the shell CLI answers it
//! (`cli/src/app-doctor.sh`, `cli/src/app-runtime.sh`): `app doctor`
//! without `--store` (the box doctor, or `<app>`'s reach doctor), `app
//! box` (its doctor, `podman uninstall`, `unprep`, `auto-prepare
//! on|off|status|retry` and `prepare`), `app engine install|status`,
//! `app rehost [status|policy]` and `app sessions --live`.
//!
//! - The box doctor's exit is the box's `ready`; the reach doctor's is
//!   its `ok` (missing: ok).
//! - `box unprep` without `--yes` shows the plan; at a terminal (stdin
//!   and stdout) it asks `[y/N]`, else says to pass `--yes`, exit 2.
//! - `box prepare` asks before it installs K2's Podman only at a
//!   terminal, on the daemon's 409 `podman_install_needed`.
//! - `app engine` and `app rehost` alone print the runtime page.

use super::call::{Ctx, Split};
use super::deploy::{value, Given};
use super::py::{self, Out};
use super::runtime::rt_abs;
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_iter, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// `_app_rt_usage`'s page.
pub const RUNTIME_PAGE: &str = "app/_runtime";

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const DOCTOR_TAIL: &str = concat!(" (", cli_name!(), " app doctor --help)");
    pub const DOCTOR_VOICE: &str = concat!(
        cli_name!(),
        " app doctor: --voice and --no-voice go with --store (",
        cli_name!(),
        " app doctor --store --help)"
    );
    pub const ENGINE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app engine install [--from DIR] [--json] | ",
        cli_name!(),
        " app engine status [--json]"
    );
    pub const REHOST_POLICY: &str = concat!(
        "usage: ",
        cli_name!(),
        " app rehost policy <app> [--policy P] [--window HH:MM-HH:MM] [--when-no-overlap pause|defer]"
    );
    pub const SESSIONS_TAIL: &str = concat!(
        " (",
        cli_name!(),
        " app sessions --live [--count] [--json])"
    );
    pub const SESSIONS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app sessions --live [--count] [--json]  (one person's sessions: ",
        cli_name!(),
        " app user sessions <username> --app <app>)"
    );
    pub const PODMAN: &str = concat!("usage: ", cli_name!(), " app box podman uninstall [--json]");
    pub const AUTO_PREPARE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app box auto-prepare on|off|status|retry [--json]"
    );
    pub const RUNTIME: &str = "--runtime is crun or youki";
    pub const SIZES: &str = "sizes are plain numbers (MiB, tasks)";
    pub const UNPREP_YES: &str = concat!(
        cli_name!(),
        " app box unprep: pass --yes to undo the box setup without a prompt (--dry-run shows the plan)"
    );
    pub const BOX_HEAD: &str = concat!("Unknown: ", cli_name!(), " app box ");
    pub const BOX_TAIL: &str =
        " (doctor|prepare|unprep|auto-prepare on|off|status|retry|podman uninstall)";
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The runtime page on stdout.
    Page,
    BoxDoctor {
        json: bool,
    },
    Reach {
        app: String,
        json: bool,
    },
    EngineInstall {
        body: String,
        json: bool,
    },
    EngineStatus {
        json: bool,
    },
    RehostStatus {
        query: String,
        json: bool,
    },
    RehostPolicy {
        body: String,
        json: bool,
    },
    Rehost {
        app: String,
        body: String,
        json: bool,
    },
    Sessions {
        count: bool,
        json: bool,
    },
    Podman {
        json: bool,
    },
    Unprep {
        dry: bool,
        yes: bool,
        json: bool,
    },
    AutoPrepare {
        verb: String,
        json: bool,
    },
    Prepare(Prepare),
}

/// `box prepare`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prepare {
    pub runtime: String,
    pub cores: String,
    pub mem: String,
    pub tasks: String,
    pub install: bool,
    pub dry: bool,
    pub from: String,
    pub json: bool,
}

/// The verbs here (`doctor` comes through [`parse_doctor`]).
pub const VERBS: &[&str] = &["box", "engine", "rehost", "sessions"];

fn help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

fn unknown_arg(w: &str) -> Stop {
    Stop::usage(format!("Unknown argument: {w}"))
}

/// `--json` and nothing else (`-h` is the caller's).
fn only_json(words: &[&str]) -> Result<bool, Stop> {
    let mut json = false;
    for w in words {
        if *w == "--json" {
            json = true;
        } else {
            return Err(unknown_arg(w));
        }
    }
    Ok(json)
}

/// `cmd_app_doctor` without `--store`. `None`: a help form or `--store`
/// ([`super::init`]'s).
pub fn parse_doctor(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) || words.contains(&"--store") {
        return None;
    }
    let (mut json, mut voice, mut folder) = (false, false, "");
    for w in words {
        match *w {
            "--json" => json = true,
            "--voice" | "--no-voice" => voice = true,
            _ if w.starts_with('-') => {
                return Some(Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::DOCTOR_TAIL
                ))))
            }
            _ if folder.is_empty() => folder = w,
            _ => {
                return Some(Err(Stop::usage(format!(
                    "Unexpected argument: {w}{}",
                    usage::DOCTOR_TAIL
                ))))
            }
        }
    }
    if voice {
        return Some(Err(Stop::usage(usage::DOCTOR_VOICE)));
    }
    Some(Ok(if folder.is_empty() {
        Ask::BoxDoctor { json }
    } else {
        Ask::Reach {
            app: folder.to_string(),
            json,
        }
    }))
}

/// The words after `box`, `engine`, `rehost` or `sessions`. `None`: a
/// help form, or `box`'s doctor (the caller's, as `doctor`).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    let first = words.first().copied();
    let rest = words.get(1..).unwrap_or_default();
    Some(match (verb, first) {
        ("engine", None | Some("")) | ("rehost", None | Some("")) => Ok(Ask::Page),
        ("engine", Some("install")) => engine_install(rest),
        ("engine", Some("status")) => only_json(rest).map(|json| Ask::EngineStatus { json }),
        ("engine", Some(_)) => Err(Stop::usage(usage::ENGINE)),
        ("rehost", Some("status")) => rehost_status(rest),
        ("rehost", Some("policy")) => rehost_policy(rest),
        ("rehost", _) => rehost(words),
        ("sessions", _) => sessions(words),
        (_, None | Some("" | "doctor" | "status")) => return None,
        (_, Some("podman")) => {
            if rest.first() != Some(&"uninstall") {
                Err(Stop::usage(usage::PODMAN))
            } else {
                only_json(&rest[1..]).map(|json| Ask::Podman { json })
            }
        }
        (_, Some("unprep")) => unprep(rest),
        (_, Some("auto-prepare")) => match rest.first().copied() {
            Some(v @ ("retry" | "on" | "off" | "status")) => {
                only_json(&rest[1..]).map(|json| Ask::AutoPrepare {
                    verb: v.to_string(),
                    json,
                })
            }
            _ => Err(Stop::usage(usage::AUTO_PREPARE)),
        },
        (_, Some("prepare")) => prepare(rest),
        (_, Some(w)) => Err(Stop::usage(format!(
            "{}{w}{}",
            usage::BOX_HEAD,
            usage::BOX_TAIL
        ))),
    })
}

fn engine_install(words: &[&str]) -> Result<Ask, Stop> {
    let (mut from, mut json) = (String::new(), false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--from" => {
                let (v, n) = value(words, i)?;
                (from, i) = (v.to_string(), n);
            }
            "--json" => (json, i) = (true, i + 1),
            _ => return Err(unknown_arg(w)),
        }
    }
    if !from.is_empty() {
        from = rt_abs(&from);
    }
    let body = Given::new().put("from", &from).body();
    Ok(Ask::EngineInstall { body, json })
}

fn rehost_status(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json) = ("", false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_arg(w)),
            _ => app = w,
        }
    }
    let query = if app.is_empty() {
        String::new()
    } else {
        format!("app={}", quote(app))
    };
    Ok(Ask::RehostStatus { query, json })
}

fn rehost_policy(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut policy, mut window, mut nooverlap, mut json) = ("", "", "", "", false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--policy" => (policy, i) = value(words, i)?,
            "--window" => (window, i) = value(words, i)?,
            "--when-no-overlap" => (nooverlap, i) = value(words, i)?,
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with('-') => return Err(unknown_arg(w)),
            _ => (app, i) = (w, i + 1),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::REHOST_POLICY));
    }
    let body = Given::new()
        .put("app", app)
        .put("policy", policy)
        .put("window", window)
        .put("whenNoOverlap", nooverlap)
        .body();
    Ok(Ask::RehostPolicy { body, json })
}

fn rehost(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut yes, mut pause, mut json) = ("", "", "", false);
    for w in words {
        match *w {
            "--yes" => yes = "1",
            "--pause" => pause = "1",
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_arg(w)),
            _ => app = w,
        }
    }
    let body = Given::new()
        .put("app", app)
        .put("yes_b", yes)
        .put("pause_b", pause)
        .body();
    Ok(Ask::Rehost {
        app: app.to_string(),
        body,
        json,
    })
}

fn sessions(words: &[&str]) -> Result<Ask, Stop> {
    let (mut live, mut count, mut json) = (false, false, false);
    for w in words {
        match *w {
            "--live" => live = true,
            "--count" => count = true,
            "--json" => json = true,
            _ => {
                return Err(Stop::usage(format!(
                    "Unknown argument: {w}{}",
                    usage::SESSIONS_TAIL
                )))
            }
        }
    }
    if !live {
        return Err(Stop::usage(usage::SESSIONS));
    }
    Ok(Ask::Sessions { count, json })
}

fn unprep(words: &[&str]) -> Result<Ask, Stop> {
    let (mut dry, mut yes, mut json) = (false, false, false);
    for w in words {
        match *w {
            "--dry-run" => dry = true,
            "--yes" | "-y" => yes = true,
            "--json" => json = true,
            _ => return Err(unknown_arg(w)),
        }
    }
    Ok(Ask::Unprep { dry, yes, json })
}

fn prepare(words: &[&str]) -> Result<Ask, Stop> {
    let mut p = Prepare::default();
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let take = |slot: &mut String| -> Result<usize, Stop> {
            let (v, n) = value(words, i)?;
            *slot = v.to_string();
            Ok(n)
        };
        i = match *w {
            "--runtime" => take(&mut p.runtime)?,
            "--cores-mem-min" => take(&mut p.cores)?,
            "--apps-mem-max" => take(&mut p.mem)?,
            "--apps-tasks-max" => take(&mut p.tasks)?,
            "--podman-from" => take(&mut p.from)?,
            "--youki" => {
                p.runtime = "youki".into();
                i + 1
            }
            "--install-podman" | "--yes" | "-y" => {
                p.install = true;
                i + 1
            }
            "--dry-run" => {
                p.dry = true;
                i + 1
            }
            "--json" => {
                p.json = true;
                i + 1
            }
            _ => return Err(unknown_arg(w)),
        };
    }
    if !matches!(p.runtime.as_str(), "" | "crun" | "youki") {
        return Err(Stop::usage(usage::RUNTIME));
    }
    for n in [&p.cores, &p.mem, &p.tasks] {
        if !n.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Stop::usage(usage::SIZES));
        }
    }
    if !p.from.is_empty() {
        p.from = rt_abs(&p.from);
        p.install = true;
    }
    Ok(Ask::Prepare(p))
}

fn post(ctx: &mut Ctx, path: &str, body: &str, secs: Option<u64>) -> Result<Split, Exit> {
    match secs {
        Some(s) => ctx.post_max(path, body, s),
        None => ctx.send(true, path, body, false),
    }
}

/// The POST, then `[ "$_CP_STATUS" = "200" ] || _compute_fail`.
fn post_ok(ctx: &mut Ctx, path: &str, body: &str, secs: Option<u64>) -> Result<Vec<u8>, Exit> {
    let s = post(ctx, path, body, secs)?;
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

type Printer = fn(&[u8], &mut Out) -> Result<Exit, Raise>;

/// `--json`: the answer as it came; else the printer.
fn shown(body: &[u8], json: bool, lines: Printer) -> Shown {
    super::access::answer(body, json, lines)
}

/// `if python3 -c 'sys.exit(0 if … else 1)'; then return 0; fi; exit 1`:
/// a raise is its traceback, and exit 1.
fn exit_by(o: &mut Out, r: Result<bool, Raise>) -> Exit {
    match r {
        Ok(true) => Exit::Ok,
        Ok(false) => Exit::Error,
        Err(e) => {
            o.stderr.push_str(&e.text());
            Exit::Error
        }
    }
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Page => Ok(Shown::out(
            crate::text::help(RUNTIME_PAGE).unwrap_or_default(),
        )),
        Ask::BoxDoctor { json } => {
            let body = ctx.call(false, "/cli/app/box", "")?;
            let mut o = Out::default();
            if *json {
                o.stdout.push_str(&raw(&body).stdout);
            } else if let Err(e) = box_lines(&body, &mut o) {
                return Ok(o.done(Err(e)));
            }
            let r = (|| {
                let v = py::load(&body, 1)?;
                Ok(py::truthy(&py::dget(&v, "ready", 1)?))
            })();
            let exit = exit_by(&mut o, r);
            Ok(o.done(Ok(exit)))
        }
        Ask::Reach { app, json } => {
            let body = ctx.call(
                false,
                "/cli/app/box-service",
                &format!("app={}", quote(app)),
            )?;
            let mut o = Out::default();
            let r = if *json {
                reach_json(&body, &mut o)
            } else {
                reach_lines(&body, &mut o)
            };
            if let Err(e) = r {
                return Ok(o.done(Err(e)));
            }
            let r = (|| {
                let v = py::load(&body, 1)?;
                let reach = py::or(py::dget(&v, "reach", 1)?, Value::Object(Vec::new()));
                Ok(
                    if py::truthy(&py::dget_or(&reach, "ok", Value::Bool(true), 1)?) {
                        Exit::Ok
                    } else {
                        Exit::Error
                    },
                )
            })();
            Ok(o.done(r))
        }
        Ask::EngineInstall { body, json } => {
            let body = post_ok(ctx, "/cli/app/engine/install", body, Some(600))?;
            Ok(shown(&body, *json, engine_install_lines))
        }
        Ask::EngineStatus { json } => {
            let body = ctx.call(false, "/cli/app/engine/status", "")?;
            Ok(shown(&body, *json, engine_status_lines))
        }
        Ask::RehostStatus { query, json } => {
            let body = ctx.call(false, "/cli/app/rehost", query)?;
            Ok(shown(&body, *json, rehost_status_lines))
        }
        Ask::RehostPolicy { body, json } => {
            let body = post_ok(ctx, "/cli/app/rehost/policy", body, None)?;
            Ok(shown(&body, *json, rehost_policy_lines))
        }
        Ask::Rehost { app, body, json } => {
            let body = post_ok(ctx, "/cli/app/rehost/run", body, Some(1800))?;
            if *json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = rehost_lines(&body, &mut o, app);
            Ok(o.done(r))
        }
        Ask::Sessions { count, json } => {
            let body = ctx.call(false, "/cli/app/sessions/live", "")?;
            if *json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = sessions_lines(&body, &mut o, *count);
            Ok(o.done(r))
        }
        Ask::Podman { json } => {
            let body = post_ok(ctx, "/cli/app/box/podman/uninstall", "{}", Some(300))?;
            Ok(shown(&body, *json, podman_lines))
        }
        Ask::Unprep { dry, yes, json } => unprep_run(ctx, *dry, *yes, *json),
        Ask::AutoPrepare { verb, json } => auto_prepare(ctx, verb, *json),
        Ask::Prepare(p) => prepare_run(ctx, p),
    }
}

fn unprep_run(ctx: &mut Ctx, dry: bool, yes: bool, json: bool) -> Answer {
    let mut o = Out::default();
    if dry || !yes {
        let body = post_ok(ctx, "/cli/app/box/unprep", r#"{"dryRun":true}"#, Some(120))?;
        if json && dry {
            return Ok(raw(&body));
        }
        if let Err(e) = unprep_plan_lines(&body, &mut o) {
            return Ok(o.done(Err(e)));
        }
        if dry {
            o.line("Dry run: nothing changed.");
            return Ok(o.done(Ok(Exit::Ok)));
        }
        if !(crate::prompt::stdin_is_tty() && crate::prompt::stdout_is_tty()) {
            o.err(usage::UNPREP_YES);
            return Ok(o.done(Ok(Exit::Usage)));
        }
        let printed = o.done(Ok(Exit::Ok)).print();
        if printed != Exit::Ok {
            return Err(printed);
        }
        o = Out::default();
        if !ask_yes("Undo the box setup? [y/N] ") {
            out::stderr_line("Nothing changed.");
            return Err(Exit::Usage);
        }
    }
    let body = post_ok(ctx, "/cli/app/box/unprep", r#"{"confirm":true}"#, Some(300))?;
    if json {
        return Ok(raw(&body));
    }
    let r = unprep_done_lines(&body, &mut o).and_then(|_| box_lines(&body, &mut o));
    Ok(o.done(r))
}

/// `printf '<q>'; read -r yn || yn=""`: a yes?
fn ask_yes(q: &str) -> bool {
    {
        use std::io::Write;
        let mut so = std::io::stdout().lock();
        let _ = so.write_all(q.as_bytes());
        let _ = so.flush();
    }
    let line = crate::prompt::read_line_or_end(crate::prompt::Echo::Shown);
    let yn = line.map(|l| super::deploy::ifs_trim(l.expose()).to_vec());
    matches!(yn.as_deref(), Some(b"y" | b"Y" | b"yes" | b"YES"))
}

fn auto_prepare(ctx: &mut Ctx, verb: &str, json: bool) -> Answer {
    match verb {
        "status" => {
            let body = ctx.call(false, "/cli/app/box", "")?;
            let mut o = Out::default();
            let r = if json {
                (|| {
                    let v = py::load(&body, 1)?;
                    let ap = py::dget(&v, "autoPrepare", 1)?;
                    o.line(&crate::confirm::dumps(&ap));
                    Ok(Exit::Ok)
                })()
            } else {
                auto_status_lines(&body, &mut o)
            };
            Ok(o.done(r))
        }
        "on" | "off" => {
            let b = format!("{{\"on\":{}}}", verb == "on");
            let body = post_ok(ctx, "/cli/app/box/auto-prepare", &b, None)?;
            Ok(shown(&body, json, auto_on_lines))
        }
        _ => {
            let body = post_ok(ctx, "/cli/app/box/auto-prepare/retry", "{}", None)?;
            if json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            o.line("Automatic box prep will run again now.");
            let r = (|| {
                let v = py::load(&body, 3)?;
                let ap = py::or(py::dget(&v, "autoPrepare", 3)?, Value::Object(Vec::new()));
                o.line(&format!(
                    "Automatic prep: {}",
                    if py::truthy(&py::dget(&ap, "enabled", 4)?) {
                        "on"
                    } else {
                        "off (appsAutoPrepare): turn it on for the retry to do anything"
                    }
                ));
                Ok(Exit::Ok)
            })();
            Ok(o.done(r))
        }
    }
}

fn prepare_body(p: &Prepare, install: bool, full: bool) -> String {
    let one = |b: bool| if b { "1" } else { "" };
    let g = Given::new()
        .put("runtime", &p.runtime)
        .put("coresMemMinMib_n", &p.cores)
        .put("appsMemMaxMib_n", &p.mem)
        .put("appsTasksMax_n", &p.tasks)
        .put("installPodman_b", one(install));
    if full {
        g.put("dryRun_b", one(p.dry))
            .put("podmanFrom", &p.from)
            .body()
    } else {
        g.body()
    }
}

/// The 409 that asks to install K2's Podman.
fn podman_needed(body: &[u8]) -> bool {
    (|| {
        let v = py::load(body, 1)?;
        let e = py::dget_or(&v, "error", Value::Object(Vec::new()), 1)?;
        Ok::<bool, Raise>(py::dget(&e, "code", 1)? == py::text("podman_install_needed"))
    })()
    .unwrap_or(false)
}

fn prepare_run(ctx: &mut Ctx, p: &Prepare) -> Answer {
    let mut s = post(
        ctx,
        "/cli/app/box/prepare",
        &prepare_body(p, p.install, true),
        Some(900),
    )?;
    if s.status == "409"
        && !p.json
        && !p.install
        && crate::prompt::stdin_is_tty()
        && crate::prompt::stdout_is_tty()
        && podman_needed(&s.body)
    {
        let mut o = Out::default();
        let r = podman_offer_lines(&s.body, &mut o);
        let printed = o.done(r).print();
        if printed != Exit::Ok {
            return Err(printed);
        }
        if !ask_yes("Install it now? [y/N] ") {
            out::stderr_line("Nothing changed.");
            return Err(Exit::Usage);
        }
        s = post(
            ctx,
            "/cli/app/box/prepare",
            &prepare_body(p, true, false),
            Some(900),
        )?;
    }
    let mut o = Out::default();
    if p.json {
        o.stdout.push_str(&raw(&s.body).stdout);
    }
    if s.ok() && p.dry && !p.json {
        let r = prepare_dry_lines(&s.body, &mut o).and_then(|_| box_lines(&s.body, &mut o));
        return Ok(o.done(r));
    }
    if !s.ok() {
        if !p.json {
            let mut steps = Out::default();
            let r = prepare_fail_lines(&s.body, &mut steps);
            let shown = steps.done(r);
            // Its stdout went to stderr; a raise ends the command (set -e).
            o.stderr.push_str(&shown.stdout);
            o.stderr.push_str(&shown.stderr);
            if shown.exit != Exit::Ok {
                return Ok(o.done(Ok(Exit::Error)));
            }
        }
        let printed = o.done(Ok(Exit::Ok)).print();
        if printed != Exit::Ok {
            return Err(printed);
        }
        return Err(ctx.fail(&s));
    }
    if !p.json {
        let r = prepare_steps_lines(&s.body, &mut o)
            .and_then(|_| box_lines(&s.body, &mut o))
            .and_then(|_| restart_line(&s.body, &mut o));
        return Ok(o.done(r));
    }
    Ok(o.done(Ok(Exit::Ok)))
}

/// `_app_box_print`.
pub fn box_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let b = py::dget_or(&v, "box", v.clone(), 4)?;
    let linux = py::dget_or(&b, "linux", Value::Bool(true), 5)? != Value::Bool(false);
    if linux {
        let ready = if py::truthy(&py::dget(&b, "ready", 7)?) {
            "ready"
        } else {
            "NOT READY"
        };
        let runtime = py::dget_or(&b, "runtime", py::text("?"), 7)?;
        let default = if py::truthy(&py::dget(&b, "runtimeRecorded", 8)?) {
            ""
        } else {
            ", default"
        };
        o.line(&format!(
            "App box: {ready} (runtime {}{default})",
            py::s(&runtime)
        ));
    } else {
        o.line(
            "App box: not a Linux box. Apps run on Linux only; there is no box prep to do here.",
        );
    }
    for c in py_iter(&py::dget_or(&b, "checks", py::empty_list(), 11)?, 11)? {
        let status = py::dget_or(&c, "status", py::text("?"), 12)?;
        let mut line = format!(
            "  {} {} {}",
            py::left(&status, 5),
            py::left(&py::dget_or(&c, "id", py::text(""), 12)?, 17),
            py::s(&py::dget_or(&c, "detail", py::text(""), 12)?)
        );
        let st = py::dget(&c, "status", 13)?;
        if py::truthy(&py::dget(&c, "fix", 13)?)
            && ["fail", "warn", "unknown"]
                .iter()
                .any(|k| py_eq(&st, &py::text(k)))
        {
            line.push_str(&py::concat("  -> ", &py::idx(&c, "fix", 14)?, 14)?);
        }
        o.line(&line);
    }
    let ap = py::dget(&v, "autoPrepare", 17)?;
    if matches!(ap, Value::Object(_)) && linux {
        let oc = py::or(py::dget(&ap, "outcome", 19)?, Value::Object(Vec::new()));
        let st = py::or(py::dget(&oc, "state", 20)?, py::text("not run yet"));
        let mut extra = py::or(py::dget(&oc, "why", 21)?, py::dget(&oc, "error", 21)?);
        if !py::truthy(&extra) {
            let changed = py::or(py::dget(&oc, "changed", 21)?, py::empty_list());
            extra = py::text(&py::join(", ", &changed, 21)?);
        }
        let tail = if py::truthy(&extra) {
            py::concat(" \u{2014} ", &extra, 22)?
        } else {
            String::new()
        };
        o.line(&format!(
            "Automatic prep: {}; last: {}{tail}",
            if py::truthy(&py::dget(&ap, "enabled", 22)?) {
                "on"
            } else {
                "off (appsAutoPrepare)"
            },
            py::s(&st)
        ));
        if py::truthy(&py::dget(&ap, "undoneAt", 23)?) {
            o.line(concat!(
                "  paused: the owner undid the box setup (",
                cli_name!(),
                " app box auto-prepare retry resumes it)"
            ));
        } else if py::truthy(&py::dget(&ap, "blocked", 25)?) || st == py::text("blocked") {
            o.line(concat!(
                "  stopped for good: no retry until ",
                product_name!(),
                " updates (",
                cli_name!(),
                " app box auto-prepare retry)"
            ));
        }
    }
    let legacy = py::or(py::dget(&v, "legacyServices", 28)?, py::empty_list());
    if py::truthy(&legacy) {
        o.line(
            "Legacy --cmd services (unwalled; they keep running, move each to an App when ready):",
        );
        for l in py_iter(&legacy, 32)? {
            o.line(&format!(
                "  {} {} {}  -> {}",
                py::left(&py::dget(&l, "name", 33)?, 20),
                py::left(
                    &py::or(py::dget(&l, "workspaceHandle", 33)?, py::text("")),
                    14
                ),
                py::s(&py::dget_or(&l, "label", py::text(""), 33)?),
                py::s(&py::dget_or(&l, "hint", py::text(""), 33)?)
            ));
        }
    }
    Ok(Exit::Ok)
}

/// `str(p) for p in xs` joined by `, `.
fn joined_str(xs: &Value, line: u32) -> Result<String, Raise> {
    Ok(py_iter(xs, line)?
        .iter()
        .map(py::s)
        .collect::<Vec<_>>()
        .join(", "))
}

fn reach_json(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    let r = py::or(py::dget(&v, "reach", 1)?, Value::Object(Vec::new()));
    let mut pairs = match r {
        Value::Object(p) => p,
        Value::Array(_) => {
            return Err(Raise::at(
                1,
                "TypeError: list indices must be integers or slices, not str",
            ))
        }
        other => {
            return Err(Raise::at(
                1,
                format!(
                    "TypeError: '{}' object does not support item assignment",
                    crate::out::py::type_name(&other)
                ),
            ))
        }
    };
    for k in ["app", "wall"] {
        let val = py::dget(&v, k, 1)?;
        let key = crate::out::json::Text::from(k);
        match pairs.iter_mut().find(|(n, _)| *n == key) {
            Some(slot) => slot.1 = val,
            None => pairs.push((key, val)),
        }
    }
    o.line(&crate::confirm::dumps(&Value::Object(pairs)));
    Ok(Exit::Ok)
}

fn reach_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::or(py::dget(&v, "reach", 4)?, Value::Object(Vec::new()));
    let state = if py::truthy(&py::dget(&r, "running", 5)?) {
        "running"
    } else {
        "not running"
    };
    let app = py::dget(&v, "app", 6)?;
    let wall = py::dget(&v, "wall", 6)?;
    let ok = if py::truthy(&py::dget(&r, "ok", 6)?) {
        "everything approved is applied"
    } else {
        "NOT ALL APPLIED"
    };
    o.line(&format!(
        "{} ({}, {state}): {ok}",
        py::s(&app),
        py::s(&wall)
    ));
    let rule = py::dget(&r, "rule", 7)?;
    if py::truthy(&rule) {
        let db = py::dget(&rule, "dbPorts", 9)?;
        let gw = py::dget(&rule, "gwPort", 10)?;
        let boxp = joined_str(
            &py::or(py::dget(&rule, "boxPorts", 10)?, py::empty_list()),
            10,
        )?;
        let pg = if db == Value::Null {
            "the whole band".to_string()
        } else {
            let t = joined_str(&db, 11)?;
            if t.is_empty() {
                "none".into()
            } else {
                t
            }
        };
        o.line(&format!(
            "  rule      gateway {}, box ports {}, Postgres {pg}",
            py::s(&gw),
            if boxp.is_empty() { "none" } else { &boxp }
        ));
    }
    let door = py::dget(&r, "dbDoor", 12)?;
    if py::truthy(&door) {
        let port = py::dget(&door, "inWallPort", 14)?;
        let sessions = py::dget(&door, "openSessions", 14)?;
        let dbs = py::join(
            ", ",
            &py::or(py::dget(&door, "databases", 15)?, py::empty_list()),
            15,
        )?;
        o.line(&format!(
            "  db door   127.0.0.1:{} in the wall, {} open session(s), databases: {}",
            py::s(&port),
            py::s(&sessions),
            if dbs.is_empty() { "none" } else { &dbs }
        ));
    }
    for f in py_iter(&py::or(py::dget(&r, "findings", 16)?, py::empty_list()), 16)? {
        o.line(&format!(
            "  {} {}",
            py::left(&py::dget(&f, "id", 17)?, 26),
            py::s(&py::dget(&f, "detail", 17)?)
        ));
    }
    Ok(Exit::Ok)
}

fn engine_install_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let e = py::dget_or(&v, "engine", Value::Object(Vec::new()), 4)?;
    let g = |k: &str| py::dget(&e, k, 5).map(|x| py::s(&x));
    o.line(&format!(
        "k2-app-engine {} installed and answering (protocol {}, runtime {}, {})",
        g("version")?,
        g("protocol")?,
        g("runtime")?,
        g("podman")?
    ));
    Ok(Exit::Ok)
}

fn engine_status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let b = |x: Value, line: u32| -> Result<String, Raise> {
        if py::truthy(&x) {
            Ok(format!(
                "v{} protocol {}",
                py::s(&py::dget(&x, "version", 5).map_err(|e| Raise::inside(
                    line,
                    5,
                    "b",
                    e.exception
                ))?),
                py::s(&py::dget(&x, "protocol", 5).map_err(|e| Raise::inside(
                    line,
                    5,
                    "b",
                    e.exception
                ))?)
            ))
        } else {
            Ok("-".into())
        }
    };
    if py::dget(&v, "state", 6)? == py::text("absent") {
        o.line("App engine: not installed on this box");
        return Ok(Exit::Ok);
    }
    o.line(&format!(
        "App engine: {}",
        py::s(&py::dget(&v, "state", 9)?)
    ));
    o.line(&format!(
        concat!("  this ", product_name!(), ":   {}"),
        b(py::dget(&v, "want", 10)?, 10)?
    ));
    o.line(&format!(
        "  installed: {}",
        b(py::dget(&v, "installed", 11)?, 11)?
    ));
    o.line(&format!(
        "  running:   {}",
        b(py::dget(&v, "running", 12)?, 12)?
    ));
    o.line(&format!(
        "  linger:    {}",
        if py::truthy(&py::dget(&v, "linger", 13)?) {
            "on"
        } else {
            "OFF (Apps stop at the next k2app logout)"
        }
    ));
    let h = py::or(py::dget(&v, "heal", 14)?, Value::Object(Vec::new()));
    if py::truthy(&h) {
        let why = if py::truthy(&py::dget(&h, "why", 16)?) {
            py::concat(": ", &py::idx(&h, "why", 16)?, 16)?
        } else {
            String::new()
        };
        o.line(&format!(
            "  heal:      {}{why}",
            py::s(&py::dget(&h, "state", 16)?)
        ));
    }
    let apps = py::or(py::dget(&v, "apps", 17)?, Value::Object(Vec::new()));
    let w = py::or(
        py::dget(&apps, "waitingEngine", 17)?,
        Value::Int("0".into()),
    );
    if py::truthy(&w) {
        o.line(&format!(
            "  waiting:   {} App(s) wait for the engine",
            py::d(&w, 0, 19)?
        ));
    }
    Ok(Exit::Ok)
}

/// `{"overlap": a, "pause": b}.get(key, default)`: a key Python can't
/// hash is its `TypeError`.
fn by_mode(key: &Value, a: &str, b: &str, line: u32) -> Result<Option<String>, Raise> {
    match key {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!(
                "TypeError: unhashable type: '{}'",
                crate::out::py::type_name(key)
            ),
        )),
        k if *k == py::text("overlap") => Ok(Some(a.to_string())),
        k if *k == py::text("pause") => Ok(Some(b.to_string())),
        _ => Ok(None),
    }
}

fn rehost_status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::contains(&v, "rehost", 4)? {
        let r = py::idx(&v, "rehost", 5)?;
        let fp = py::or(py::dget(&r, "fingerprint", 5)?, Value::Object(Vec::new()));
        let app = py::idx(&v, "app", 6)?;
        o.line(&format!(
            "{}: policy {}, when it can't run two copies: {}",
            py::s(&app),
            py::s(&py::dget(&r, "policy", 6)?),
            py::s(&py::dget(&r, "whenNoOverlap", 6)?)
        ));
        o.line(&format!(
            concat!("  runs on: ", product_name!(), " {}, engine {}"),
            py::s(&py::dget_or(&fp, "k2Version", py::text("?"), 7)?),
            py::s(&py::dget_or(&fp, "engineVersion", py::text("-"), 7)?)
        ));
        o.line(&format!(
            "  due: {}; sessions: {}",
            py::s(&py::or(py::dget(&r, "due", 8)?, py::text("no"))),
            py::s(&py::dget(&r, "sessions", 8)?)
        ));
        let w = py::or(py::dget(&r, "wave", 9)?, Value::Object(Vec::new()));
        if py::truthy(&w) {
            let code = if py::truthy(&py::dget(&w, "code", 11)?) {
                format!("{})", py::concat(" (", &py::idx(&w, "code", 11)?, 11)?)
            } else {
                String::new()
            };
            o.line(&format!(
                "  wave: {}{code}",
                py::s(&py::dget(&w, "state", 11)?)
            ));
            if py::truthy(&py::dget(&w, "detail", 12)?) {
                o.line(&py::concat("    ", &py::idx(&w, "detail", 13)?, 13)?);
            }
            if py::truthy(&py::dget(&w, "mode", 14)?) {
                let mode = py::idx(&w, "mode", 15)?;
                let how = by_mode(&mode, "no downtime", "stopped briefly", 15)?
                    .unwrap_or_else(|| "?".into());
                let why = if py::truthy(&py::dget(&w, "forced", 15)?) {
                    concat!("asked for with ", cli_name!(), " app rehost")
                } else {
                    concat!("started by ", product_name!(), " after an update")
                };
                o.line(&format!("  last re-host: {} ({how}), {why}", py::s(&mode)));
            }
        }
        return Ok(Exit::Ok);
    }
    let reason = if py::truthy(&py::dget(&v, "reason", 17)?) {
        py::concat(" \u{2014} ", &py::idx(&v, "reason", 17)?, 17)?
    } else {
        String::new()
    };
    o.line(&format!(
        concat!("Re-host wave (", product_name!(), " {}): {}{}"),
        py::s(&py::dget(&v, "k2Version", 17)?),
        py::s(&py::dget(&v, "state", 17)?),
        reason
    ));
    let apps = py::or(py::dget(&v, "apps", 18)?, Value::Object(Vec::new()));
    let mut pairs = match apps {
        Value::Object(p) => p,
        other => return Err(Raise::no_attribute(18, &other, "items")),
    };
    pairs.sort_by(|a, b| py::s(&Value::Str(a.0.clone())).cmp(&py::s(&Value::Str(b.0.clone()))));
    for (a, s) in pairs {
        let detail = if py::truthy(&py::dget(&s, "detail", 19)?) {
            py::concat(": ", &py::idx(&s, "detail", 19)?, 19)?
        } else {
            String::new()
        };
        o.line(&format!(
            "  {} {} {}{detail}",
            py::left(&Value::Str(a), 24),
            py::left(&py::dget(&s, "state", 19)?, 9),
            py::s(&py::or(py::dget(&s, "code", 19)?, py::text("")))
        ));
    }
    Ok(Exit::Ok)
}

fn rehost_policy_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::idx(&v, "rehost", 3)?;
    let app = py::idx(&v, "app", 4)?;
    let policy = py::idx(&r, "policy", 4)?;
    let window = if py::truthy(&py::dget(&r, "window", 4)?) {
        py::concat(" ", &py::idx(&r, "window", 4)?, 4)?
    } else {
        String::new()
    };
    let nooverlap = py::idx(&r, "whenNoOverlap", 4)?;
    o.line(&format!(
        "{}: policy {}{window}; when it can't run two copies: {}",
        py::s(&app),
        py::s(&policy),
        py::s(&nooverlap)
    ));
    if py::truthy(&py::dget(&v, "warning", 5)?) {
        o.line(&py::concat("warning: ", &py::idx(&v, "warning", 6)?, 6)?);
    }
    Ok(Exit::Ok)
}

fn rehost_lines(body: &[u8], o: &mut Out, arg: &str) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::dget(&v, "rehost", 4)?;
    let app = py::s(&py::or(py::dget(&v, "app", 5)?, py::text(arg)));
    let how = by_mode(
        &py::dget(&v, "mode", 6)?,
        "the new instance started beside the old one and took over, with no downtime",
        "the App was stopped briefly (two copies of it can not run at once), then started on the new instance",
        6,
    )?;
    if r == py::text("current") {
        o.line(&format!(
            concat!(
                "{} already runs on this ",
                product_name!(),
                " (nothing to move)"
            ),
            app
        ));
    } else if r == py::text("done") {
        let d = py::or(py::dget(&v, "deployment", 10)?, Value::Object(Vec::new()));
        o.line(&format!(
            "{app} moved to a fresh instance ({})",
            py::s(&py::dget(&d, "id", 10)?)
        ));
        if let Some(how) = how {
            o.line(&format!(
                "  how: {}: {how}",
                py::s(&py::idx(&v, "mode", 12)?)
            ));
        }
    } else {
        o.line(&format!(
            "{app}: the re-host failed; it keeps running on its previous instance: {}",
            py::s(&py::dget(&v, "why", 14)?)
        ));
        if how.is_some() {
            let mode = py::idx(&v, "mode", 16)?;
            let tried = by_mode(
                &mode,
                "a new instance beside the old one",
                "stop first, then start",
                16,
            )?
            .unwrap_or_default();
            o.line(&format!("  how it was tried: {} ({tried})", py::s(&mode)));
        }
        o.line(&format!(
            concat!(
                "  why: ",
                cli_name!(),
                " app history {} ; try again: ",
                cli_name!(),
                " app rehost {}"
            ),
            app, app
        ));
        return Ok(Exit::Error);
    }
    Ok(Exit::Ok)
}

fn sessions_lines(body: &[u8], o: &mut Out, count: bool) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let zero = || Value::Int("0".into());
    let c = py::dget_or(&v, "connected", zero(), 4)?;
    let s = py::dget_or(&v, "signedIn", zero(), 4)?;
    let one = py_eq(&c, &Value::Int("1".into()));
    o.line(&format!(
        "{} guest{} connected now, {} signed in",
        py::d(&c, 0, 5)?,
        if one { "" } else { "s" },
        py::d(&s, 0, 5)?
    ));
    if count {
        return Ok(Exit::Ok);
    }
    for a in py_iter(&py::or(py::dget(&v, "apps", 8)?, py::empty_list()), 8)? {
        o.line(&format!(
            "  {} {} connected  {} signed in",
            py::left(&py::dget(&a, "app", 9)?, 24),
            py::d(&py::dget_or(&a, "connected", zero(), 9)?, 4, 9)?,
            py::d(&py::dget_or(&a, "signedIn", zero(), 9)?, 4, 9)?
        ));
    }
    if py_eq(&c, &zero()) {
        o.line("Nobody is connected now: a restart signs nobody out.");
    }
    Ok(Exit::Ok)
}

fn podman_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::dget_or(&v, "result", Value::Object(Vec::new()), 3)?;
    let gone = py::or(py::dget(&r, "removed", 4)?, py::empty_list());
    let what = if py::truthy(&gone) {
        py::join(", ", &gone, 5)?
    } else {
        "nothing was installed".to_string()
    };
    o.line(&format!(
        concat!(product_name!(), " Podman removed: {}"),
        what
    ));
    for k in py_iter(&py::or(py::dget(&r, "kept", 6)?, py::empty_list()), 6)? {
        o.line(&py::concat("  kept ", &k, 7)?);
    }
    o.line("The distro Podman is as it was.");
    Ok(Exit::Ok)
}

fn unprep_plan_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    o.line("Undo box setup would remove:");
    for u in py_iter(&py::or(py::dget(&v, "undoes", 5)?, py::empty_list()), 5)? {
        o.line(&py::concat("  ", &u, 6)?);
    }
    for k in py_iter(&py::or(py::dget(&v, "kept", 7)?, py::empty_list()), 7)? {
        o.line(&py::concat("  keeps ", &k, 8)?);
    }
    let run = py::or(py::dget(&v, "runningApps", 9)?, py::empty_list());
    if py::truthy(&run) {
        o.line(&format!(
            "Refused while Apps run; stop these first: {}",
            py::join(", ", &run, 11)?
        ));
    }
    Ok(Exit::Ok)
}

fn unprep_done_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::or(py::dget(&v, "result", 4)?, Value::Object(Vec::new()));
    o.line("Box setup undone:");
    for s in py_iter(&py::or(py::dget(&r, "steps", 6)?, py::empty_list()), 6)? {
        let step = py::dget_or(&s, "step", py::text(""), 7)?;
        let ch = if py::truthy(&py::dget(&s, "changed", 7)?) {
            "  (changed)"
        } else {
            "  (already)"
        };
        o.line(&format!("  ok   {}{ch}", py::s(&step)));
    }
    let mut kept = py::dget(&v, "kept", 8)?;
    if !py::truthy(&kept) {
        kept = py::or(py::dget(&r, "kept", 8)?, py::empty_list());
    }
    for k in py_iter(&kept, 8)? {
        o.line(&py::concat("  kept ", &k, 9)?);
    }
    if py::truthy(&py::dget(&v, "note", 10)?) {
        o.line(&py::s(&py::idx(&v, "note", 11)?));
    }
    Ok(Exit::Ok)
}

fn auto_status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let ap = py::or(py::dget(&v, "autoPrepare", 3)?, Value::Object(Vec::new()));
    o.line(&format!(
        "Automatic prep: {}",
        if py::truthy(&py::dget(&ap, "enabled", 4)?) {
            "on"
        } else {
            "off"
        }
    ));
    let oc = py::or(py::dget(&ap, "outcome", 5)?, Value::Object(Vec::new()));
    if py::truthy(&oc) {
        let why = py::or(py::dget(&oc, "why", 7)?, py::dget(&oc, "error", 7)?);
        let tail = if py::truthy(&why) {
            py::concat(": ", &why, 7)?
        } else {
            String::new()
        };
        o.line(&format!(
            "  last run: {}{tail}",
            py::s(&py::dget(&oc, "state", 7)?)
        ));
    }
    Ok(Exit::Ok)
}

fn auto_on_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let ap = py::or(py::dget(&v, "autoPrepare", 4)?, Value::Object(Vec::new()));
    let on = py::truthy(&py::dget(&ap, "enabled", 5)?);
    o.line(&format!(
        "Automatic prep: {} (appsAutoPrepare {})",
        if on { "on" } else { "off" },
        if py::truthy(&py::dget(&v, "setting", 5)?) {
            "on"
        } else {
            "off"
        }
    ));
    if py::truthy(&py::dget(&v, "note", 6)?) {
        o.line(&py::concat("  ", &py::idx(&v, "note", 7)?, 7)?);
    }
    if py::truthy(&py::dget(&ap, "enabled", 8)?) {
        o.line(concat!(
            "  ",
            product_name!(),
            " keeps this server set up for Apps through its root helper, after every update. ",
            cli_name!(),
            " app box auto-prepare off turns it off."
        ));
    }
    Ok(Exit::Ok)
}

fn podman_offer_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let e = py::dget_or(&v, "error", Value::Object(Vec::new()), 3)?;
    let p = py::dget_or(&e, "podman", Value::Object(Vec::new()), 4)?;
    let b = py::dget_or(&p, "bundle", Value::Object(Vec::new()), 5)?;
    o.line(&py::s(&py::dget_or(&e, "hint", py::text(""), 6)?));
    o.line(&format!(
        concat!("  ", product_name!(), " Podman {} ({}), sha256-pinned:"),
        py::s(&py::dget(&b, "version", 7)?),
        py::s(&py::dget(&b, "source", 7)?)
    ));
    o.line(&format!("    from   {}", py::s(&py::dget(&b, "url", 8)?)));
    for w in py_iter(&py::dget_or(&b, "writes", py::empty_list(), 9)?, 9)? {
        o.line(&format!("    writes {}", py::s(&w)));
    }
    o.line(&format!(
        "    {}",
        py::s(&py::dget_or(&b, "distroPodman", py::text(""), 11)?)
    ));
    Ok(Exit::Ok)
}

fn prepare_dry_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let p = py::dget_or(&v, "podman", Value::Object(Vec::new()), 4)?;
    o.line("Dry run: nothing changed.");
    let a = py::dget(&p, "action", 6)?;
    let have_or_none = |line: u32| -> Result<String, Raise> {
        Ok(py::s(&py::or(
            py::dget(&p, "have", line)?,
            py::text("none"),
        )))
    };
    if a == py::text("keep") {
        o.line(&format!(
            "  Podman {} ({}): nothing to install",
            py::s(&py::dget(&p, "have", 8)?),
            py::s(&py::dget(&p, "source", 8)?)
        ));
    } else if a == py::text("distro") {
        o.line(&format!(
            "  Podman {}: {}",
            have_or_none(10)?,
            py::s(&py::dget_or(&p, "note", py::text(""), 10)?)
        ));
    } else {
        let b = py::dget_or(&p, "bundle", Value::Object(Vec::new()), 12)?;
        let have = have_or_none(13)?;
        let os = py::s(&py::dget(&p, "os", 13)?);
        let verb = if a == py::text("install") {
            "would install"
        } else {
            "needs --install-podman to install"
        };
        o.line(&format!(
            concat!("  Podman {} on {}: {} ", product_name!(), " Podman {} ({})"),
            have,
            os,
            verb,
            py::s(&py::dget(&b, "version", 13)?),
            py::s(&py::dget(&b, "source", 13)?)
        ));
        o.line(&format!("    from   {}", py::s(&py::dget(&b, "url", 14)?)));
        o.line(&format!(
            "    sha256 {} ({} bytes)",
            py::s(&py::dget(&b, "sha256", 15)?),
            py::s(&py::dget(&b, "size", 15)?)
        ));
        for w in py_iter(&py::dget_or(&b, "writes", py::empty_list(), 16)?, 16)? {
            o.line(&format!("    writes {}", py::s(&w)));
        }
    }
    let k = py::or(py::dget(&v, "packages", 18)?, Value::Object(Vec::new()));
    if py::truthy(&py::dget(&k, "manager", 19)?) {
        if py::truthy(&py::dget(&k, "error", 20)?) {
            o.line(&format!(
                "  Distro packages ({}): {}",
                py::s(&py::idx(&k, "manager", 21)?),
                py::s(&py::idx(&k, "error", 21)?)
            ));
        } else {
            let inst = py::or(py::dget(&k, "install", 23)?, py::empty_list());
            let m = py::idx(&k, "manager", 24)?;
            let what = if py::truthy(&inst) {
                py::join(" ", &inst, 24)?
            } else {
                "none, all already installed".to_string()
            };
            o.line(&format!(
                "  Distro packages prepare would install ({}): {what}",
                py::s(&m)
            ));
            if py::truthy(&py::dget(&k, "alreadyInstalled", 25)?) {
                o.line(&format!(
                    "    already installed: {}",
                    py::join(" ", &py::idx(&k, "alreadyInstalled", 26)?, 26)?
                ));
            }
        }
        if py::truthy(&py::dget(&k, "distroPodman", 27)?) {
            o.line(&format!(
                "  Podman: {}",
                py::s(&py::idx(&k, "distroPodman", 28)?)
            ));
        }
    } else if py::truthy(&k) {
        o.line(&format!(
            "  Distro packages: none ({})",
            py::s(&py::dget_or(
                &k,
                "note",
                py::text("no package manager"),
                30
            )?)
        ));
    }
    if py::truthy(&py::dget(&k, "manager", 31)?) && py::truthy(&py::dget(&k, "note", 31)?) {
        o.line(&format!("    {}", py::s(&py::idx(&k, "note", 32)?)));
    }
    o.line("Steps prepare would run:");
    for s in py_iter(&py::dget_or(&v, "steps", py::empty_list(), 34)?, 34)? {
        o.line(&py::concat("  ", &s, 35)?);
    }
    Ok(Exit::Ok)
}

/// The failed prepare's steps (printed to stderr). Nothing when the
/// answer can't be read.
fn prepare_fail_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let Ok(v) = py::load(body, 4) else {
        return Ok(Exit::Ok);
    };
    let Ok(e) = py::dget_or(&v, "error", Value::Object(Vec::new()), 4) else {
        return Ok(Exit::Ok);
    };
    for s in py_iter(&py::dget_or(&e, "steps", py::empty_list(), 7)?, 7)? {
        let ok = py::truthy(&py::dget(&s, "ok", 8)?);
        let verb = py::dget_or(&s, "verb", py::text(""), 8)?;
        let err = if ok {
            String::new()
        } else {
            format!(": {}", py::s(&py::dget_or(&s, "error", py::text(""), 8)?))
        };
        o.line(&format!(
            "  {} {}{err}",
            super::super::ljust(if ok { "ok" } else { "FAIL" }, 4),
            py::s(&verb)
        ));
    }
    Ok(Exit::Ok)
}

fn prepare_steps_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    for s in py_iter(&py::dget_or(&v, "steps", py::empty_list(), 4)?, 4)? {
        let r = py::dget(&s, "result", 5)?;
        let is_dict = matches!(r, Value::Object(_));
        let ch = if is_dict {
            py::dget(&r, "changed", 6)?
        } else {
            Value::Null
        };
        let tail = if ch == Value::Null {
            ""
        } else if py::truthy(&ch) {
            "  (changed)"
        } else {
            "  (already)"
        };
        o.line(&format!(
            "  ok   {}{tail}",
            py::s(&py::dget_or(&s, "verb", py::text(""), 7)?)
        ));
        let aa = if is_dict {
            py::dget(&r, "apparmor", 8)?
        } else {
            Value::Null
        };
        if matches!(aa, Value::Object(_)) && py::truthy(&py::dget(&aa, "needed", 9)?) {
            if py::truthy(&py::dget(&aa, "installed", 10)?) {
                o.line(&format!(
                    "       AppArmor: installed the distro package {} (apparmor_parser {}) for the k2-app profile",
                    py::join(" ", &py::idx(&aa, "installed", 11)?, 11)?,
                    py::s(&py::dget(&aa, "parser", 11)?)
                ));
            } else if py::truthy(&py::dget(&aa, "note", 12)?) {
                o.line(&format!(
                    "       AppArmor: {}",
                    py::s(&py::idx(&aa, "note", 13)?)
                ));
            }
        }
    }
    for n in py_iter(&py::or(py::dget(&v, "notes", 14)?, py::empty_list()), 14)? {
        o.line(&format!("Note: {}", py::s(&n)));
    }
    Ok(Exit::Ok)
}

/// 0.46.16: the agents leaf waits for the daemon's next start.
fn restart_line(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let b = py::or(py::dget(&v, "box", 3)?, Value::Object(Vec::new()));
    let mut rows: Vec<(Value, Value)> = Vec::new();
    for c in py_iter(&py::or(py::dget(&b, "checks", 4)?, py::empty_list()), 4)? {
        let id = py::dget(&c, "id", 4)?;
        let status = py::dget(&c, "status", 4)?;
        if matches!(id, Value::Array(_) | Value::Object(_)) {
            return Err(Raise::at(
                4,
                format!(
                    "TypeError: unhashable type: '{}'",
                    crate::out::py::type_name(&id)
                ),
            ));
        }
        match rows.iter_mut().find(|(k, _)| py_eq(k, &id)) {
            Some(slot) => slot.1 = status,
            None => rows.push((id, status)),
        }
    }
    let get = |k: &str| {
        rows.iter()
            .find(|(id, _)| *id == py::text(k))
            .map_or(Value::Null, |(_, s)| s.clone())
    };
    let leaf = get("agentsLeaf");
    if py_eq(&get("coresSlice"), &py::text("ok"))
        && !(leaf == Value::Null || py_eq(&leaf, &py::text("ok")))
    {
        o.line(concat!(
            "Restart ",
            product_name!(),
            " once so agent sessions get their own memory budget: ",
            cli_name!(),
            " daemon restart (it signs guests out of older App sites; ",
            cli_name!(),
            " app sessions --live shows who is on now), or pick Tonight at 3 am on the Apps setup card."
        ));
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(verb: &str, w: &[&str]) -> Stop {
        match parse(verb, w) {
            Some(Err(s)) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_words_as_the_shell_reads_them() {
        assert_eq!(parse("box", &[]), None);
        assert_eq!(parse("box", &["status", "x"]), None);
        assert_eq!(parse("engine", &[]), Some(Ok(Ask::Page)));
        assert_eq!(stop("engine", &["x"]).line, usage::ENGINE);
        assert_eq!(
            stop("engine", &["install", "a"]).line,
            "Unknown argument: a"
        );
        assert_eq!(stop("rehost", &["policy"]).line, usage::REHOST_POLICY);
        assert_eq!(stop("sessions", &[]).line, usage::SESSIONS);
        assert_eq!(stop("box", &["podman"]).line, usage::PODMAN);
        assert_eq!(
            stop("box", &["auto-prepare", "x"]).line,
            usage::AUTO_PREPARE
        );
        assert_eq!(
            stop("box", &["prepare", "--runtime", "runc"]).line,
            usage::RUNTIME
        );
        assert_eq!(
            stop("box", &["prepare", "--apps-mem-max", "1G"]).line,
            usage::SIZES
        );
        assert_eq!(
            stop("box", &["prepare", "--runtime"]),
            Stop::silent(Exit::Error)
        );
        assert!(stop("box", &["zap"]).line.ends_with("podman uninstall)"));
        assert_eq!(parse_doctor(&["--store"]), None);
        assert_eq!(
            parse_doctor(&["--voice"])
                .and_then(Result::err)
                .map(|s| s.line),
            Some(usage::DOCTOR_VOICE.to_string())
        );
        assert_eq!(
            parse_doctor(&["a", "b"])
                .and_then(Result::err)
                .map(|s| s.exit),
            Some(Exit::Usage)
        );
    }
}
