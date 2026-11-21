//! The App runtime's change verbs, as the shell CLI answers them
//! (`cli/src/app-doctor.sh`): `app build`, `app install` (not
//! `--catalog`: [`super::catalog`]), `app deploy`, `app rollback`, `app stop|start|restart`,
//! `app link`, `app seal` and `app config`.
//!
//! Each reads its words as the shell's loop does (a usage mistake is its
//! line and exit 2 before anything is sent; a flag's value missing is
//! `_need_value`'s line, exit 2), sends one
//! request (`_app_rt_post` or `_compute_call`), and prints the answer as
//! the shell's Python does, step by step: a step that raises ends the
//! command with its traceback and exit 1.
//!
//! - A 422 from `build` or `deploy` is the validate report on stderr and
//!   exit 1 (`build --json`: the answer as it came, on stdout).
//! - A held change (202) is `_app_grace_done` ([`super::grace`]): its
//!   lines and exit 2.
//! - `_app_rt_deployment_exit`: the deployment's line, then the exit its
//!   result says: live 0, pending or starting 2, stalled 3, else 1.
//! - `seal rotate` without `--yes` asks for the App id at a terminal
//!   (stdin and stderr), else says how and exits 2.

use super::call::{Cj, Ctx, Given, Split};
use super::py::{self, Out};
use super::runtime::{rt_abs, rt_abs_file};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_iter, Raise, Shown};
use crate::out::json::Value;
use crate::out::pylib;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const BUILD: &str = concat!(
        "usage: ",
        cli_name!(),
        " app build <folder> [--manifest <file>] [--types] [--disk-override REASON] [--json]"
    );
    pub const INSTALL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app install <app-id> [--workspace WS] [--tunnel [--transfer]] [--memory 8G|none] [--engine guarded|process] [--seed S|--folder F] [--pg-major 16|17]"
    );
    pub const PG_MAJOR: &str = "usage: --pg-major takes a Postgres major, e.g. 17";
    pub const DEPLOY: &str = concat!(
        "usage: ",
        cli_name!(),
        " app deploy <app-id> <folder|seed> [--manifest <file>] [--engine guarded|process] [--reason TEXT] [--no-wait] [--disk-override REASON]"
    );
    pub const ENGINE: &str = "--engine is guarded or process";
    pub const DEPLOY_MANIFEST: &str = concat!(
        cli_name!(),
        " app deploy: --manifest goes with a folder (a stored Seed already has its manifest)"
    );
    pub const ROLLBACK: &str =
        concat!("usage: ", cli_name!(), " app rollback <app-id> [--to SEED]");
    pub const LINK: &str = concat!(
        "usage: ",
        cli_name!(),
        " app link <app-id> [--rotate] [--json]"
    );
    pub const SEAL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app seal <app-id> | ",
        cli_name!(),
        " app seal rotate <app-id> [--yes]"
    );
    pub const CONFIG: &str = concat!(
        "usage: ",
        cli_name!(),
        " app config <app-id> --memory 8G|512M|none [--egress add HOST:PORT] \u{2026} (",
        cli_name!(),
        " app --help)"
    );
    pub const CONFIG_FLAG_TAIL: &str = concat!(" (", cli_name!(), " app --help)");
    pub const EGRESS: &str = "usage: --egress add|rm HOST:PORT";
    pub const PORT: &str = "usage: --port add|rm PORT[/tcp|udp]";
    pub const IO: &str = "--io is default or none";
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Build {
        body: String,
        json: bool,
    },
    /// `install`: the pairs but the workspace, which the run names
    /// (`_compute_ws`).
    Install {
        app: String,
        ws: String,
        rest: Given,
        json: bool,
    },
    Deploy {
        body: String,
        json: bool,
    },
    Rollback {
        body: String,
        json: bool,
    },
    /// `stop`, `start` or `restart`.
    Life {
        verb: String,
        body: String,
        json: bool,
    },
    Link {
        app: String,
        rotate: bool,
        json: bool,
    },
    Seal {
        app: String,
        rotate: bool,
        yes: bool,
        json: bool,
    },
    Config {
        body: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &[
    "build", "install", "deploy", "rollback", "stop", "start", "restart", "link", "seal", "config",
];

/// `x="${2:-}"; shift 2` at `i`: the value and the next index, or
/// `_need_value`'s line (no value).
pub fn value<'a>(words: &[&'a str], i: usize) -> Result<(&'a str, usize), Stop> {
    match words.get(i + 1) {
        Some(v) => Ok((v, i + 2)),
        None => Err(Stop::need_value(words[i])),
    }
}

fn help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

fn is_dir(p: &str) -> bool {
    !p.is_empty() && std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

fn is_file(p: &str) -> bool {
    !p.is_empty() && std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

fn bit(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

fn unknown(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

/// `cmd_app_install`'s first look: a word `--catalog` (or
/// `--catalog=…`) anywhere makes it the one-click install
/// ([`super::catalog::parse_install`]).
pub fn catalog_install(words: &[&str]) -> bool {
    words
        .iter()
        .any(|w| *w == "--catalog" || w.starts_with("--catalog="))
}

/// The words after the verb (`install --catalog` is
/// [`super::catalog`]'s). `None`: a help form.
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(match verb {
        "build" => build(words),
        "install" => install(words),
        "deploy" => deploy(words),
        "rollback" => rollback(words),
        "link" => link(words),
        "seal" => seal(words),
        "config" => config(words),
        _ => life(verb, words),
    })
}

fn build(words: &[&str]) -> Result<Ask, Stop> {
    let (mut folder, mut types, mut json, mut dov, mut manifest) = ("", false, false, "", "");
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--manifest" => (manifest, i) = value(words, i)?,
            "--types" => (types, i) = (true, i + 1),
            "--disk-override" => (dov, i) = value(words, i)?,
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with("--manifest=") => (manifest, i) = (&w[11..], i + 1),
            _ if w.starts_with("--disk-override=") => (dov, i) = (&w[16..], i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => (folder, i) = (w, i + 1),
        }
    }
    if folder.is_empty() {
        return Err(Stop::usage(usage::BUILD));
    }
    if !is_dir(folder) {
        return Err(Stop::usage(format!(
            concat!(cli_name!(), " app build: {} is not a folder"),
            folder
        )));
    }
    let mut m = String::new();
    if !manifest.is_empty() {
        if !is_file(manifest) {
            return Err(Stop::usage(format!(
                concat!(cli_name!(), " app build: --manifest {} is not a file"),
                manifest
            )));
        }
        m = rt_abs_file(manifest);
    }
    let body = Given::new()
        .put("folder", &rt_abs(folder))
        .put("types_b", bit(types))
        .put("diskOverride", dov)
        .put("manifest", &m)
        .body();
    Ok(Ask::Build { body, json })
}

fn install(words: &[&str]) -> Result<Ask, Stop> {
    let mut g = Given::new();
    let (mut app, mut ws, mut json, mut pg, mut waits) = ("", "", false, "", false);
    let (mut tunnel, mut transfer) = ("0", "");
    let (mut memory, mut engine, mut seed, mut folder, mut dov) = ("", "", "", String::new(), "");
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--transfer" => (transfer, i) = ("1", i + 1),
            "--disk-override" => (dov, i) = value(words, i)?,
            "--workspace" => (ws, i) = value(words, i)?,
            "--tunnel" => (tunnel, i) = ("1", i + 1),
            "--memory" => (memory, i) = value(words, i)?,
            "--engine" => (engine, i) = value(words, i)?,
            "--seed" => (seed, i) = value(words, i)?,
            "--folder" => {
                let (f, next) = value(words, i)?;
                (folder, i) = (rt_abs(f), next);
            }
            "--pg-major" => (pg, i) = value(words, i)?,
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with("--disk-override=") => (dov, i) = (&w[16..], i + 1),
            _ if w.starts_with("--pg-major=") => (pg, i) = (&w[11..], i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => (app, i) = (w, i + 1),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::INSTALL));
    }
    if !pg.is_empty() {
        let b = pg.as_bytes();
        if !(b.len() == 2 && (b'1'..=b'9').contains(&b[0]) && b[1].is_ascii_digit()) {
            return Err(Stop::usage(usage::PG_MAJOR));
        }
    }
    if !seed.is_empty() || !folder.is_empty() {
        waits = true;
    }
    g = g
        .put("tunnel_b", tunnel)
        .put("memory", memory)
        .put("engine", engine)
        .put("seed", seed)
        .put("folder", &folder)
        .put("wait_n", if waits { "150" } else { "" })
        .put("diskOverride", dov)
        .put("pgMajor_n", pg)
        .put("transfer_b", transfer);
    Ok(Ask::Install {
        app: app.to_string(),
        ws: ws.to_string(),
        rest: g,
        json,
    })
}

fn deploy(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut target, mut reason, mut wait) = ("", "", "", "150");
    let (mut types, mut json, mut engine, mut dov, mut manifest) = (false, false, "", "", "");
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--disk-override" => (dov, i) = value(words, i)?,
            "--manifest" => (manifest, i) = value(words, i)?,
            "--engine" => (engine, i) = value(words, i)?,
            "--reason" => (reason, i) = value(words, i)?,
            "--no-wait" => (wait, i) = ("0", i + 1),
            "--types" => (types, i) = (true, i + 1),
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with("--disk-override=") => (dov, i) = (&w[16..], i + 1),
            _ if w.starts_with("--manifest=") => (manifest, i) = (&w[11..], i + 1),
            _ if w.starts_with("--engine=") => (engine, i) = (&w[9..], i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => {
                if app.is_empty() {
                    app = w;
                } else {
                    target = w;
                }
                i += 1;
            }
        }
    }
    if app.is_empty() || target.is_empty() {
        return Err(Stop::usage(usage::DEPLOY));
    }
    if !matches!(engine, "" | "guarded" | "process") {
        return Err(Stop::usage(usage::ENGINE));
    }
    let mut m = String::new();
    if !manifest.is_empty() {
        if !is_dir(target) {
            return Err(Stop::usage(usage::DEPLOY_MANIFEST));
        }
        if !is_file(manifest) {
            return Err(Stop::usage(format!(
                concat!(cli_name!(), " app deploy: --manifest {} is not a file"),
                manifest
            )));
        }
        m = rt_abs_file(manifest);
    }
    let g = Given::new().put("app", app);
    let g = if is_dir(target) {
        g.put("folder", &rt_abs(target))
            .put("reason", reason)
            .put("wait_n", wait)
            .put("types_b", bit(types))
            .put("engine", engine)
            .put("diskOverride", dov)
            .put("manifest", &m)
    } else {
        g.put("seed", target)
            .put("reason", reason)
            .put("wait_n", wait)
            .put("engine", engine)
            .put("diskOverride", dov)
    };
    Ok(Ask::Deploy {
        body: g.body(),
        json,
    })
}

fn rollback(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut to, mut wait, mut json) = ("", "", "150", false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--to" => (to, i) = value(words, i)?,
            "--no-wait" => (wait, i) = ("0", i + 1),
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => (app, i) = (w, i + 1),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::ROLLBACK));
    }
    let body = Given::new()
        .put("app", app)
        .put("to", to)
        .put("wait_n", wait)
        .body();
    Ok(Ask::Rollback { body, json })
}

/// The words of a verb that takes `<app-id>` and the flags in `flags`
/// (each set when present). The App, or the unknown flag.
fn app_and<'a>(words: &[&'a str], flags: &[&str]) -> Result<(&'a str, Vec<bool>), Stop> {
    let mut app = "";
    let mut set = vec![false; flags.len()];
    for w in words {
        if let Some(n) = flags.iter().position(|f| f == w) {
            set[n] = true;
        } else if w.starts_with('-') {
            return Err(unknown(w));
        } else {
            app = w;
        }
    }
    Ok((app, set))
}

fn life(verb: &str, words: &[&str]) -> Result<Ask, Stop> {
    let (app, set) = app_and(words, &["--json"])?;
    if app.is_empty() {
        return Err(Stop::usage(format!(
            concat!("usage: ", cli_name!(), " app {} <app-id>"),
            verb
        )));
    }
    let wait = if verb == "stop" { "" } else { "150" };
    let body = Given::new().put("app", app).put("wait_n", wait).body();
    Ok(Ask::Life {
        verb: verb.to_string(),
        body,
        json: set[0],
    })
}

fn link(words: &[&str]) -> Result<Ask, Stop> {
    let (app, set) = app_and(words, &["--rotate", "--json"])?;
    if app.is_empty() {
        return Err(Stop::usage(usage::LINK));
    }
    Ok(Ask::Link {
        app: app.to_string(),
        rotate: set[0],
        json: set[1],
    })
}

fn seal(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut rotate, mut yes, mut json) = ("", false, false, false);
    for w in words {
        match *w {
            "rotate" => rotate = true,
            "--yes" | "-y" => yes = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => app = w,
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::SEAL));
    }
    Ok(Ask::Seal {
        app: app.to_string(),
        rotate,
        yes,
        json,
    })
}

fn config(words: &[&str]) -> Result<Ask, Stop> {
    let mut kv = Given::new();
    let (mut app, mut json) = ("", false);
    let (mut egress_add, mut egress_rm, mut port_add, mut port_rm) =
        (String::new(), String::new(), String::new(), String::new());
    let (mut io_r, mut io_w, mut io_ri, mut io_wi) = ("", "", "", "");
    // `${list:+$list,}${3:-}`.
    let add = |list: &mut String, v: &str| {
        if !list.is_empty() {
            list.push(',');
        }
        list.push_str(v);
    };
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let key = match *w {
            "--memory" => Some("memory"),
            "--cpu-weight" => Some("cpuWeight"),
            "--tasks" => Some("tasks"),
            "--disk" => Some("disk"),
            "--egress-mode" => Some("egressMode"),
            "--recycle-every" => Some("recycleEvery"),
            "--recycle-at" => Some("recycleAt"),
            "--isolation" => Some("isolation"),
            "--path" => Some("path"),
            "--risk" => Some("risk"),
            "--db-private" => Some("dbPrivate"),
            "--db-raw" => Some("dbRaw"),
            _ => None,
        };
        if let Some(key) = key {
            let v = words.get(i + 1).copied().unwrap_or("");
            kv = kv.put(key, v);
            (_, i) = value(words, i)?;
            continue;
        }
        match *w {
            "--egress" | "--port" => {
                let third = words.get(i + 2).copied().unwrap_or("");
                let (a, r) = if *w == "--egress" {
                    (&mut egress_add, &mut egress_rm)
                } else {
                    (&mut port_add, &mut port_rm)
                };
                match words.get(i + 1).copied().unwrap_or("") {
                    "add" => add(a, third),
                    "rm" => add(r, third),
                    _ if *w == "--egress" => return Err(Stop::usage(usage::EGRESS)),
                    _ => return Err(Stop::usage(usage::PORT)),
                }
                if i + 3 > words.len() {
                    return Err(Stop::silent(Exit::Error));
                }
                i += 3;
            }
            "--io" => {
                let v = words.get(i + 1).copied().unwrap_or("");
                kv = kv.put("io", v);
                if !matches!(v, "default" | "none") {
                    return Err(Stop::usage(usage::IO));
                }
                (_, i) = value(words, i)?;
            }
            "--io-read" => (io_r, i) = value(words, i)?,
            "--io-write" => (io_w, i) = value(words, i)?,
            "--io-read-iops" => (io_ri, i) = value(words, i)?,
            "--io-write-iops" => (io_wi, i) = value(words, i)?,
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::CONFIG_FLAG_TAIL
                )))
            }
            _ => (app, i) = (w, i + 1),
        }
    }
    for (k, v) in [
        ("egressAdd", &egress_add),
        ("egressRm", &egress_rm),
        ("portAdd", &port_add),
        ("portRm", &port_rm),
    ] {
        if !v.is_empty() {
            kv = kv.put(k, v);
        }
    }
    if !(io_r.is_empty() && io_w.is_empty() && io_ri.is_empty() && io_wi.is_empty()) {
        let mut o = Cj::new();
        for (k, v) in [
            ("rbps", io_r),
            ("wbps", io_w),
            ("riops", io_ri),
            ("wiops", io_wi),
        ] {
            o = o.s(k, v);
        }
        kv = kv.put("io_j", &o.body());
    }
    if app.is_empty() || kv.is_empty() {
        return Err(Stop::usage(usage::CONFIG));
    }
    let body = Given::new().put("app", app).then(&kv).body();
    Ok(Ask::Config { body, json })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Build { body, json } => {
            let s = ctx.send(true, "/cli/app/build", body, false)?;
            if s.status == "422" {
                if *json {
                    let mut shown = raw(&s.body);
                    shown.exit = Exit::Error;
                    return Ok(shown);
                }
                return Ok(report_on_stderr(&s.body));
            }
            let body = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            if *json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            disk_notes(&body, &mut o);
            let r = build_lines(&body, &mut o);
            Ok(o.done(r))
        }
        Ask::Install {
            app,
            ws,
            rest,
            json,
        } => {
            let ws = ctx.ws(ws);
            let body = Given::new()
                .put("app", app)
                .put("workspace", &ws)
                .then(rest)
                .body();
            let s = ctx.send(true, "/cli/app/install", &body, false)?;
            if s.status == "202" && !contains(&s.body, b"\"frontPort\"") {
                return held(ctx, s, *json);
            }
            if !s.ok() {
                return Err(ctx.fail(&s));
            }
            if *json {
                return Ok(raw(&s.body));
            }
            let mut o = Out::default();
            disk_notes(&s.body, &mut o);
            let r = install_lines(&s.body, &mut o);
            Ok(o.done(r))
        }
        Ask::Deploy { body, json } => {
            let s = ctx.send(true, "/cli/app/deploy", body, false)?;
            if s.status == "422" {
                return Ok(report_on_stderr(&s.body));
            }
            let body = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            let mut o = Out::default();
            if !*json {
                disk_notes(&body, &mut o);
                if let Err(e) = seed_size(&body, &mut o).and_then(|()| dev_site(&body, &mut o)) {
                    return Ok(o.done(Err(e)));
                }
            }
            Ok(deployment_exit(o, &body, *json))
        }
        Ask::Rollback { body, json } => {
            let body = post_200(ctx, "/cli/app/rollback", body)?;
            Ok(deployment_exit(Out::default(), &body, *json))
        }
        Ask::Life { verb, body, json } => {
            let body = post_200(ctx, &format!("/cli/app/{verb}"), body)?;
            let mut o = Out::default();
            // `if printf … | python3 -c '… "deployment" in …'`: a raise is
            // its traceback, and the test is false.
            let has = (|| {
                let v = py::load(&body, 1)?;
                py::contains(&v, "deployment", 1)
            })();
            match has {
                Ok(true) => return Ok(deployment_exit(o, &body, *json)),
                Ok(false) => {}
                Err(e) => {
                    let Shown { stderr, .. } =
                        crate::families::raised(String::new(), String::new(), &e);
                    o.stderr.push_str(&stderr);
                }
            }
            if *json {
                o.stdout.push_str(&raw(&body).stdout);
                return Ok(o.done(Ok(Exit::Ok)));
            }
            let r = life_lines(&body, &mut o);
            Ok(o.done(r))
        }
        Ask::Link { app, rotate, json } => {
            let body = if *rotate {
                let b = Given::new().put("app", app).body();
                post_200(ctx, "/cli/app/link/rotate", &b)?
            } else {
                ctx.call(false, "/cli/app/link", &format!("app={}", quote(app)))?
            };
            let mut o = Out::default();
            let r = if *json {
                o.stdout.push_str(&raw(&body).stdout);
                Ok(Exit::Ok)
            } else {
                link_lines(&body, &mut o)
            };
            if let Err(e) = r {
                return Ok(o.done(Err(e)));
            }
            let r = open_exit(&body, "link", "open");
            Ok(o.done(r))
        }
        Ask::Seal {
            app,
            rotate,
            yes,
            json,
        } => {
            if *rotate {
                return seal_rotate(ctx, app, *yes, *json);
            }
            let body = ctx.call(false, "/cli/app/seal", &format!("app={}", quote(app)))?;
            let mut o = Out::default();
            if *json {
                o.stdout.push_str(&raw(&body).stdout);
            } else if let Err(e) = seal_lines(&body, &mut o) {
                return Ok(o.done(Err(e)));
            }
            let r = open_exit(&body, "seal", "present");
            Ok(o.done(r))
        }
        Ask::Config { body, json } => {
            let s = ctx.send(true, "/cli/app/config", body, false)?;
            let body = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            if *json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = config_lines(&body, &mut o);
            Ok(o.done(r))
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `_app_grace_done` on a 202.
fn held(ctx: &Ctx, s: Split, json: bool) -> Answer {
    match super::grace::done(ctx, s, json) {
        Ok(_) => unreachable!("a 202 is held"),
        Err(a) => a,
    }
}

/// `_app_rt_post`, then `[ "$_CP_STATUS" = "200" ] || _compute_fail`.
pub fn post_200(ctx: &mut Ctx, path: &str, body: &str) -> Result<Vec<u8>, Exit> {
    let s = ctx.send(true, path, body, false)?;
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

/// `printf '%s' "$_CP_BODY" | _app_rt_report >&2; exit 1`. Python writes
/// its traceback first and its own lines when it ends (a pipe's buffer).
fn report_on_stderr(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = super::runtime::report_lines(body, &mut o);
    let shown = o.done(r);
    let stderr = if shown.exit == Exit::Ok {
        format!("{}{}", shown.stdout, shown.stderr)
    } else {
        format!("{}{}", shown.stderr, shown.stdout)
    };
    Shown {
        stdout: String::new(),
        stderr,
        exit: Exit::Error,
    }
}

/// `sys.exit(0 if (json.load(sys.stdin).get(<outer>) or {}).get(<key>)
/// else 1)`.
fn open_exit(body: &[u8], outer: &str, key: &str) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    let o = py::or(py::dget(&v, outer, 1)?, Value::Object(Vec::new()));
    Ok(if py::truthy(&py::dget(&o, key, 1)?) {
        Exit::Ok
    } else {
        Exit::Error
    })
}

fn is_dict(v: &Value) -> bool {
    matches!(v, Value::Object(_))
}

/// `_app_disk_notes`: never raises, never fails the command.
pub fn disk_notes(body: &[u8], o: &mut Out) {
    let Ok(v) = pylib::json::loads(&String::from_utf8_lossy(body)) else {
        return;
    };
    if std::str::from_utf8(body).is_err() || !is_dict(&v) {
        return;
    }
    let get = |v: &Value, k: &str| py::dget(v, k, 0).unwrap_or(Value::Null);
    for (key, label) in [("diskWarning", "disk"), ("memoryWarning", "memory")] {
        let w = get(&v, key);
        if is_dict(&w) && py::truthy(&get(&w, "hint")) {
            o.stderr
                .push_str(&format!("{label} warning: {}\n", py::s(&get(&w, "hint"))));
        }
    }
    let d = get(&v, "diskOverride");
    if is_dict(&d) && py::truthy(&get(&d, "applied")) {
        let or_empty = |k: &str| match &d {
            Value::Object(p) => crate::families::get(p, k)
                .cloned()
                .unwrap_or_else(|| py::text("")),
            _ => py::text(""),
        };
        o.stderr.push_str(&format!(
            "disk override ({}): {}\n",
            py::s(&or_empty("via")),
            py::s(&or_empty("reason"))
        ));
    }
}

/// `cmd_app_build`'s printers after the notes.
fn build_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::idx(&v, "seed", 4)?;
    o.line(&format!("Seed {}", py::s(&py::idx(&s, "hash", 5)?)));
    let seed_id = py::idx(&s, "seedId", 6)?;
    let version = py::or(py::dget(&s, "version", 6)?, py::text(""));
    let entries = py::dget(&v, "entries", 6)?;
    let size = py::idx(&s, "sizeBytes", 6)?;
    let reused = if py::truthy(&py::dget(&v, "reused", 7)?) {
        "  (already stored)"
    } else {
        ""
    };
    o.line(&format!(
        "  {} {}  {} files, {} bytes{reused}",
        py::s(&seed_id),
        py::s(&version),
        py::s(&entries),
        py::s(&size)
    ));
    if py::truthy(&py::dget(&s, "sourceRef", 8)?) {
        o.line(&format!(
            "  commit {}",
            py::s(&py::idx(&s, "sourceRef", 9)?)
        ));
    }
    let report = py::or(py::dget(&s, "report", 10)?, Value::Object(Vec::new()));
    for w in py::iter(&py::dget_or(&report, "warnings", py::empty_list(), 10)?, 10)? {
        o.line(&format!(
            "  warning {}: {}",
            py::s(&py::dget_or(&w, "code", py::text(""), 11)?),
            py::s(&py::dget_or(&w, "message", py::text(""), 11)?)
        ));
    }
    seed_size(body, o)?;
    let v = py::load(body, 1)?;
    let hash = py::idx(&py::idx(&v, "seed", 1)?, "hash", 1)?;
    o.line(&format!(
        concat!("Deploy it: ", cli_name!(), " app deploy <app-id> {}"),
        py::s(&py::head(&hash, 12, 1)?)
    ));
    Ok(Exit::Ok)
}

/// `h(n)` of `_app_seed_size_note`.
fn human(n: &Value, line: u32) -> Result<String, Raise> {
    let lt = |x: f64| x < 1024.0;
    let mut x = match n {
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY),
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        other => {
            return Err(Raise::at(
                12,
                format!(
                    "TypeError: '<' not supported between instances of '{}' and 'int'",
                    crate::out::py::type_name(other)
                ),
            ))
        }
    };
    if lt(x) {
        return Ok(format!("{} B", py::d(n, 0, line)?));
    }
    for unit in ["KB", "MB", "GB"] {
        x /= 1024.0;
        if lt(x) || unit == "GB" {
            return Ok(format!("{} {unit}", py::f(x, 0, 1)));
        }
    }
    unreachable!("GB ends the loop")
}

/// `_app_seed_size_note`.
fn seed_size(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let Ok(v) = py::load(body, 4) else {
        return Ok(());
    };
    let mut m = py::dget(&v, "seedSize", 7)?;
    if !py::truthy(&m) {
        let d = py::or(py::dget(&v, "deploy", 7)?, Value::Object(Vec::new()));
        m = py::dget(&d, "seedSize", 7)?;
    }
    if !is_dict(&m) {
        return Ok(());
    }
    let bytes = py::dget_or(&m, "bytes", Value::Int("0".into()), 15)?;
    let size = human(&bytes, 15)?;
    let files = py::d(
        &py::dget_or(&m, "files", Value::Int("0".into()), 15)?,
        0,
        15,
    )?;
    let ignore = py::truthy(&py::dget(&m, "ignoreFile", 15)?);
    o.line(&format!(
        "Seed: {size}, {files} files{}",
        if ignore { ", .k2appignore applied" } else { "" }
    ));
    if py::truthy(&py::dget(&m, "largest", 16)?) {
        let mut parts = Vec::new();
        for p in py_iter(&py::idx(&m, "largest", 17)?, 17)? {
            let path = py::idx(&p, "path", 17)?;
            let b = py::idx(&p, "bytes", 17)?;
            parts.push(format!("{} {}", py::s(&path), human(&b, 17)?));
        }
        o.line(&format!("  largest: {}", parts.join(", ")));
    }
    let big = match &bytes {
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY) > f64::from(20u32 << 20),
        Value::Float(f) => *f > f64::from(20u32 << 20),
        _ => false,
    };
    if !ignore && big {
        o.line("  Leave out what the App does not run: a .k2appignore at the folder root (gitignore syntax),");
        o.line(concat!(
            "  e.g. var/  .env*  node_modules/.cache  .next/cache  (",
            cli_name!(),
            " study app-deploys)"
        ));
    }
    Ok(())
}

/// `cmd_app_deploy`'s dev-site note.
fn dev_site(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let site = match py::load(body, 4) {
        Ok(v) => py::dget(&v, "devSite", 4)?,
        Err(_) => Value::Null,
    };
    if py::truthy(&site) {
        let s = py::s(&site);
        o.line(&format!(
            concat!(
                "note: the dev site {} keeps running from this folder (",
                cli_name!(),
                " publish stop {})."
            ),
            s, s
        ));
    }
    Ok(())
}

/// `_app_rt_deployment_exit "$json_out"`, after what `o` holds.
pub fn deployment_exit(mut o: Out, body: &[u8], json: bool) -> Shown {
    if json {
        o.stdout.push_str(&raw(body).stdout);
    } else if let Err(e) = deployment_lines(body, &mut o) {
        return o.done(Err(e));
    }
    let r = (|| {
        let v = py::load(body, 1)?;
        let d = py::or(py::dget(&v, "deployment", 1)?, Value::Object(Vec::new()));
        let result = py::s(&py::dget_or(&d, "result", py::text(""), 1)?);
        Ok(match result.trim_end_matches('\n') {
            "live" => Exit::Ok,
            "pending" | "starting" => Exit::Usage,
            "stalled" => Exit::Refused,
            _ => Exit::Error,
        })
    })();
    o.done(r)
}

fn deployment_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let d = py::or(py::dget(&v, "deployment", 4)?, Value::Object(Vec::new()));
    let seed = py::head(&py::or(py::dget(&d, "toSeed", 5)?, py::text("")), 12, 5)?;
    let kind = py::dget_or(&d, "kind", py::text(""), 6)?;
    let id = py::dget_or(&d, "id", py::text(""), 6)?;
    let from = py::head(&py::or(py::dget(&d, "fromSeed", 6)?, py::text("-")), 12, 6)?;
    let actor = py::dget_or(&d, "actor", py::text(""), 7)?;
    let commit = py::head(&py::or(py::dget(&d, "commitRef", 7)?, py::text("-")), 12, 7)?;
    o.line(&format!(
        "{} {}  {} \u{2192} {}  by {}  commit {}",
        py::s(&kind),
        py::s(&id),
        py::s(&from),
        py::s(&seed),
        py::s(&actor),
        py::s(&commit)
    ));
    let r = py::dget_or(&d, "result", py::text(""), 9)?;
    if r == py::text("live") {
        let port = py::s(&py::dget(&d, "port", 11)?);
        let instant = if py::truthy(&py::dget(&v, "instant", 11)?) {
            "  (instant: the previous instance was still warm)"
        } else {
            ""
        };
        o.line(&format!("live on port {port}{instant}"));
    } else if r == py::text("pending") || r == py::text("starting") {
        o.line(&format!(
            concat!(
                "{} \u{2014} not live yet; check: ",
                cli_name!(),
                " app deployment {} {}"
            ),
            py::s(&r),
            py::s(&py::dget_or(&d, "appId", py::text(""), 13)?),
            py::s(&py::dget_or(&d, "id", py::text(""), 13)?)
        ));
    } else {
        o.line(&format!(
            "{}: {}",
            py::s(&r),
            py::s(&py::or(py::dget(&d, "error", 15)?, py::text("")))
        ));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_install`'s printers after the notes.
fn install_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let app_id = py::idx(&a, "appId", 5)?;
    let front = py::idx(&v, "frontPort", 5)?;
    let limit = if py::dget(&a, "memoryLimitBytes", 6)? == Value::Null {
        "none".to_string()
    } else {
        let m = py::idx(&a, "memoryLimitBytes", 6)?;
        format!("{} MiB", py::shr(&m, 20, 6)?)
    };
    o.line(&format!(
        "App {} installed (front port {}, memory limit {limit})",
        py::s(&app_id),
        py::s(&front)
    ));
    let db = py::dget(&v, "database", 7)?;
    if py::truthy(&db) {
        if py::truthy(&py::dget(&db, "created", 9)?) {
            let name = py::idx(&db, "name", 10)?;
            let major = py::idx(&db, "major", 10)?;
            o.line(&format!(
                "database {} created on Postgres {}; DATABASE_URL reference {} (put it in .env.k2)",
                py::s(&name),
                py::s(&major),
                py::s(&py::dget(&db, "ref", 10)?)
            ));
        } else {
            let name = py::idx(&db, "name", 12)?;
            o.line(&format!(
                "database {} on Postgres {} (already there)",
                py::s(&name),
                py::s(&py::idx(&db, "major", 12)?)
            ));
        }
    }
    let dep = py::or(py::dget(&v, "deploy", 13)?, Value::Object(Vec::new()));
    let d = py::dget(&dep, "deployment", 13)?;
    if py::truthy(&d) {
        let id = py::idx(&d, "id", 15)?;
        let result = py::idx(&d, "result", 15)?;
        o.line(&format!(
            "deploy {}: {} {}",
            py::s(&id),
            py::s(&result),
            py::s(&py::or(py::dget(&d, "error", 15)?, py::text("")))
        ));
    } else {
        o.line(&format!(
            concat!("Deploy it: ", cli_name!(), " app deploy {} <folder|seed>"),
            py::s(&py::idx(&a, "appId", 17)?)
        ));
    }
    seed_size(body, o)?;
    Ok(Exit::Ok)
}

fn life_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::dget(&v, "app", 4)?;
    let mut state = py::dget(&v, "state", 4)?;
    if !py::truthy(&state) {
        state = py::or(py::dget(&v, "already", 4)?, py::text("ok"));
    }
    o.line(&format!("{}: {}", py::s(&app), py::s(&state)));
    Ok(Exit::Ok)
}

/// `t(x)` of `cmd_app_link`'s Python.
fn when(x: &Value, line: u32) -> Result<String, Raise> {
    if py::truthy(x) {
        py::local_time(x, "%Y-%m-%d %H:%M:%S", 6)
            .map_err(|e| Raise::inside(line, 6, "t", e.exception))
    } else {
        Ok("-".to_string())
    }
}

fn link_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let l = py::or(py::dget(&v, "link", 4)?, Value::Object(Vec::new()));
    let g = |k: &str, line: u32| py::dget(&l, k, line).map(|x| py::s(&x));
    let app = py::s(&py::dget(&v, "app", 8)?);
    if !py::truthy(&py::dget(&l, "open", 7)?) {
        o.line(&format!(
            concat!(
                "{}: no open gateway link (it is not running walled: ",
                cli_name!(),
                " app deploy {} <seed> --engine guarded)"
            ),
            app, app
        ));
        return Ok(Exit::Ok);
    }
    o.line(&format!(
        "{app}  gateway link {} on 127.0.0.1:{}",
        g("linkId", 10)?,
        g("port", 10)?
    ));
    o.line(&format!(
        "  in the wall  {}  (pass file {})",
        g("inWallUrl", 11)?,
        g("passFile", 11)?
    ));
    let caps = py::join(
        ", ",
        &py::or(py::dget(&l, "caps", 12)?, py::empty_list()),
        12,
    )?;
    o.line(&format!(
        "  pass caps    {}",
        if caps.is_empty() {
            "none (GET /gw/me only)"
        } else {
            &caps
        }
    ));
    let rooms = py::join(
        ", ",
        &py::or(py::dget(&l, "rooms", 13)?, py::empty_list()),
        13,
    )?;
    o.line(&format!("  rooms        {rooms}"));
    let expires = when(&py::dget(&l, "passExpiresAt", 14)?, 14)?;
    let rotates = when(&py::dget(&l, "rotateAt", 14)?, 14)?;
    o.line(&format!(
        "  pass         expires {expires}, rotates {rotates} ({} rotations, {} live)",
        g("rotations", 14)?,
        g("livePasses", 14)?
    ));
    if py::truthy(&py::dget(&l, "lastError", 15)?) {
        o.line(&format!(
            "  last error   {}",
            py::s(&py::idx(&l, "lastError", 16)?)
        ));
    }
    o.line(&format!(
        "  requests     {} (refused {}, wrong link {}), {} connected",
        g("requests", 17)?,
        g("refused", 17)?,
        g("wrongLink", 17)?,
        g("connections", 17)?
    ));
    Ok(Exit::Ok)
}

fn seal_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "seal", 4)?, Value::Object(Vec::new()));
    if !py::truthy(&py::dget(&s, "vaultOpen", 5)?) {
        o.line(&format!(
            "{}: the vault is not open ({})",
            py::s(&py::dget(&v, "app", 6)?),
            py::s(&py::dget(&s, "error", 6)?)
        ));
    } else if !py::truthy(&py::dget(&s, "present", 7)?) {
        o.line(&format!(
            concat!(
                "{}: no seal key yet (",
                product_name!(),
                " makes it at the next start)"
            ),
            py::s(&py::dget(&v, "app", 8)?)
        ));
    } else {
        let mut state = if py::truthy(&py::dget(&s, "granted", 10)?) {
            "granted"
        } else {
            "not granted yet (next start)"
        };
        if py::truthy(&py::dget(&s, "disabled", 11)?) {
            state = "disabled in the vault";
        }
        o.line(&format!(
            "{}: seal key version {}, {state}; the App reads {}",
            py::s(&py::dget(&v, "app", 13)?),
            py::s(&py::dget(&s, "version", 13)?),
            py::s(&py::dget(&s, "file", 13)?)
        ));
    }
    Ok(Exit::Ok)
}

/// `seal rotate`: the warning and the App id typed at a terminal, unless
/// `--yes`; then the POST.
fn seal_rotate(ctx: &mut Ctx, app: &str, yes: bool, json: bool) -> Answer {
    if !yes {
        crate::out::stderr_line(&format!(
            "Rotating {app}'s seal key signs EVERYONE out of it (members and guests); their passwords still work."
        ));
        if crate::prompt::stdin_is_tty() && crate::prompt::stderr_is_tty() {
            crate::prompt::prompt("Type the App id to go ahead: ");
            // `read -r typed` under `set -e`: an end before a newline ends
            // the command, saying nothing.
            let Some(typed) = crate::prompt::read_line_or_end(crate::prompt::Echo::Shown) else {
                return Err(Exit::Error);
            };
            if ifs_trim(typed.expose()) != app.as_bytes() {
                crate::out::stderr_line("Not rotated.");
                return Err(Exit::Usage);
            }
        } else {
            crate::out::stderr_line(&format!(
                concat!(
                    "Not rotated: run ",
                    cli_name!(),
                    " app seal rotate {} --yes"
                ),
                app
            ));
            return Err(Exit::Usage);
        }
    }
    let b = Given::new().put("app", app).put("yes_b", "1").body();
    let body = post_200(ctx, "/cli/app/seal/rotate", &b)?;
    if json {
        return Ok(raw(&body));
    }
    let mut o = Out::default();
    let r = (|| {
        let v = py::load(&body, 3)?;
        o.line(&format!(
            "{}: seal key rotated (version {})",
            py::s(&py::dget(&v, "app", 4)?),
            py::s(&py::dget(&v, "version", 4)?)
        ));
        o.line(&py::s(&py::dget_or(&v, "warning", py::text(""), 5)?));
        o.line(&py::s(&py::dget_or(&v, "next", py::text(""), 6)?));
        Ok(Exit::Ok)
    })();
    Ok(o.done(r))
}

/// `read -r v` with the default `IFS`: spaces, tabs and newlines off both
/// ends.
pub fn ifs_trim(line: &[u8]) -> &[u8] {
    let ws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n');
    let start = line.iter().position(|b| !ws(b)).unwrap_or(line.len());
    let end = line.iter().rposition(|b| !ws(b)).map_or(start, |e| e + 1);
    &line[start..end]
}

fn config_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    for line in py::iter(&py::or(py::dget(&v, "applied", 5)?, py::empty_list()), 5)? {
        o.line(&format!(
            "{}: {}",
            py::s(&py::idx(&a, "appId", 6)?),
            py::s(&line)
        ));
    }
    let m = py::dget(&a, "memoryLimitBytes", 7)?;
    let app_id = py::idx(&a, "appId", 8)?;
    let limit = if m == Value::Null {
        concat!(
            "none (the App may use any memory outside the ",
            product_name!(),
            " reserve)"
        )
        .to_string()
    } else {
        format!("{} MiB", py::shr(&m, 20, 8)?)
    };
    o.line(&format!("{}: memory limit {limit}", py::s(&app_id)));
    let io = py::dget(&v, "io", 9)?;
    if py::truthy(&io) {
        let f = |n: Value, iops: bool| -> Result<String, Raise> {
            if n == Value::Null {
                Ok("none".to_string())
            } else if iops {
                Ok(py::s(&n))
            } else {
                Ok(format!("{} MiB/s", py::shr(&n, 20, 11)?))
            }
        };
        let rb = f(py::dget(&io, "rbps", 12)?, false)?;
        let wb = f(py::dget(&io, "wbps", 12)?, false)?;
        let ri = f(py::dget(&io, "riops", 12)?, true)?;
        let wi = f(py::dget(&io, "wiops", 12)?, true)?;
        o.line(&format!(
            "disk ceilings: read {rb}, write {wb}, read IOPS {ri}, write IOPS {wi}"
        ));
    }
    o.line(&py::s(&py::dget_or(&v, "note", py::text(""), 13)?));
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
        assert_eq!(stop("build", &[]).line, usage::BUILD);
        assert_eq!(stop("build", &["--manifest"]), Stop::need_value("--manifest"));
        assert_eq!(stop("build", &["--x"]).line, "Unknown flag: --x");
        assert_eq!(parse("build", &["a", "-h"]), None);
        assert!(catalog_install(&["a", "--catalog=x"]));
        assert!(catalog_install(&["--seed", "--catalog"]));
        assert!(!catalog_install(&["a", "--catalogue"]));
        assert_eq!(
            stop("install", &["a", "--pg-major", "1"]).line,
            usage::PG_MAJOR
        );
        assert_eq!(
            stop("install", &["a", "--pg-major=09"]).line,
            usage::PG_MAJOR
        );
        assert_eq!(stop("deploy", &["a"]).line, usage::DEPLOY);
        assert_eq!(
            stop("deploy", &["a", "s", "--engine=x"]).line,
            usage::ENGINE
        );
        assert_eq!(stop("restart", &[]).line.split(' ').nth(3), Some("restart"));
        assert_eq!(stop("config", &["a"]).line, usage::CONFIG);
        assert_eq!(stop("config", &["a", "--egress", "x"]).line, usage::EGRESS);
        assert_eq!(
            stop("config", &["a", "--port", "add"]),
            Stop::silent(Exit::Error)
        );
        assert_eq!(stop("config", &["a", "--io", "x"]).line, usage::IO);
    }

    #[test]
    fn read_trims_as_ifs_does() {
        assert_eq!(ifs_trim(b"  acme \t\n"), b"acme");
        assert_eq!(ifs_trim(b" \n"), b"");
    }
}
