//! The rest of an App's update feed, as the shell CLI's `cmd_app_updates`
//! (`cli/src/app-updates.sh`) runs it: `app updates key` (read, or
//! `--set`/`--set-file`), `token` (read, or `--file`/`--stdin`),
//! `promote`, `rollback`, `pull` and `push` (with its wait for the
//! upload). The reads `ls`, `stats` and `fleet` are [`super::updates`]'.
//!
//! The download-host token is read from its file or stdin into the body
//! and nowhere else, never argv or the environment.

use super::call::{Cj, Ctx, Split};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_space, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};
use std::time::Duration;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const FLAG_TAIL: &str = concat!(" (", cli_name!(), " app updates --help)");
    pub const KEY: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates key <app> [--set <pubkey> | --set-file <file.pub>]"
    );
    pub const KEY_HEAD: &str = concat!(cli_name!(), " app updates key: ");
    pub const TOKEN_FLAG_TAIL: &str = concat!(
        " (",
        cli_name!(),
        " app updates --help; the token is read from --file or --stdin, never an argument)"
    );
    pub const TOKEN_TWO: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates token <app> --file <path> | --stdin (never the token itself)"
    );
    pub const TOKEN: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates token <app> --file <path> | --stdin"
    );
    pub const TOKEN_HEAD: &str = concat!(cli_name!(), " app updates token: ");
    pub const PROMOTE_TWO: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates promote <app> --from <channel> --to <channel> [--version <v>]"
    );
    pub const PROMOTE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates promote <app> --from <channel> --to <channel> [--version <v>] (or --to <channel> --version <v>)"
    );
    pub const ROLLBACK: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates rollback <app> --channel <name> --to <version>"
    );
    pub const PULL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates pull <app> --version <version>"
    );
    pub const PUSH: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates push <app> --channel <name> --version <semver> --artifact <platform>=<file> [--artifact \u{2026}] [--sig <platform>=<file.sig>] [--notes-file <file>] [--drop-platform <platform>] [--evidence <ci release run>] [--no-wait] [--json]"
    );
    pub const PUSH_HEAD: &str = concat!(cli_name!(), " app updates push: ");
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    KeyRead {
        app: String,
        json: bool,
    },
    KeySet {
        app: String,
        body: String,
        json: bool,
    },
    TokenRead {
        app: String,
        json: bool,
    },
    /// The token is read when it runs: from `file`, else stdin.
    TokenSet {
        app: String,
        file: String,
        json: bool,
    },
    Promote {
        body: String,
        json: bool,
    },
    Rollback {
        body: String,
        json: bool,
    },
    Pull {
        body: String,
        json: bool,
    },
    Push(Push),
}

/// `updates push`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Push {
    pub app: String,
    pub channel: String,
    pub version: String,
    pub body: String,
    pub wait: bool,
    pub json: bool,
}

/// The words after `updates`. `None`: a help form or a read.
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = *words.first()?;
    let rest = &words[1..];
    Some(match sub {
        "key" => key(rest),
        "token" => token(rest),
        "promote" => promote(rest),
        "rollback" => rollback(rest),
        "pull" => pull(rest),
        "push" => push(rest),
        _ => return None,
    })
}

fn unknown_flag(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}{}", usage::FLAG_TAIL))
}

/// The words: `values` are the flags that take one (`x="${2:-}"; shift
/// 2`, `_need_value`'s line when it is the last word), `switches`
/// the ones that don't; any other `-` word is `bad(w)`; a second plain
/// word is `two`. The values in order of `values`, the switches seen, and
/// the App.
struct Words {
    app: String,
    values: Vec<String>,
    switches: Vec<bool>,
}

fn words(
    ws: &[&str],
    values: &[&str],
    switches: &[&str],
    bad: fn(&str) -> Stop,
    two: &'static str,
) -> Result<Words, Stop> {
    let mut out = Words {
        app: String::new(),
        values: vec![String::new(); values.len()],
        switches: vec![false; switches.len()],
    };
    let mut rest = ws;
    while let Some(w) = rest.first().copied() {
        if let Some(i) = values.iter().position(|f| *f == w) {
            out.values[i] = rest.get(1).copied().unwrap_or("").to_string();
            if rest.len() < 2 {
                return Err(Stop::need_value(w));
            }
            rest = &rest[2..];
            continue;
        }
        if let Some(i) = switches.iter().position(|f| *f == w) {
            out.switches[i] = true;
        } else if w.starts_with('-') {
            return Err(bad(w));
        } else if out.app.is_empty() {
            out.app = w.to_string();
        } else {
            return Err(Stop::usage(two));
        }
        rest = &rest[1..];
    }
    Ok(out)
}

fn key(ws: &[&str]) -> Result<Ask, Stop> {
    // `--set` and `--set-file` say a key is being set even when given last.
    let setting = ws.iter().any(|w| matches!(*w, "--set" | "--set-file"));
    let w = words(
        ws,
        &["--set", "--set-file"],
        &["--show", "--json"],
        unknown_flag,
        usage::KEY,
    )?;
    let json = w.switches[1];
    if w.app.is_empty() {
        return Err(Stop::usage(usage::KEY));
    }
    if !setting {
        return Ok(Ask::KeyRead { app: w.app, json });
    }
    let mut pubkey = w.values[0].clone();
    let file = &w.values[1];
    if !file.is_empty() {
        if !std::path::Path::new(file).is_file() {
            return Err(Stop::usage(format!(
                "{}{file} is not a file",
                usage::KEY_HEAD
            )));
        }
        pubkey = cat(file).map_err(|line| Stop {
            line,
            exit: Exit::Error,
        })?;
    }
    if pubkey.is_empty() {
        return Err(Stop::usage(format!("{}the key is empty", usage::KEY_HEAD)));
    }
    let body = Cj::new().s("app", &w.app).s("pubkey", &pubkey).body();
    Ok(Ask::KeySet {
        app: w.app,
        body,
        json,
    })
}

/// `"$(cat "$file")"`: the text, NULs and trailing newlines gone; or
/// cat's own line.
fn cat(file: &str) -> Result<String, String> {
    let mut bytes = std::fs::read(file).map_err(|e| {
        let text = e.to_string();
        let msg = text.split(" (os error").next().unwrap_or(&text).to_string();
        format!("cat: {file}: {msg}")
    })?;
    bytes.retain(|b| *b != 0);
    while bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn token(ws: &[&str]) -> Result<Ask, Stop> {
    fn bad(w: &str) -> Stop {
        Stop::usage(format!("Unknown flag: {w}{}", usage::TOKEN_FLAG_TAIL))
    }
    let w = words(
        ws,
        &["--file"],
        &["--stdin", "--json"],
        bad,
        usage::TOKEN_TWO,
    )?;
    let (from_stdin, json) = (w.switches[0], w.switches[1]);
    if w.app.is_empty() {
        return Err(Stop::usage(usage::TOKEN));
    }
    let file = w.values[0].clone();
    if file.is_empty() && !from_stdin {
        return Ok(Ask::TokenRead { app: w.app, json });
    }
    if !file.is_empty() && !std::path::Path::new(&file).is_file() {
        return Err(Stop::usage(format!(
            "{}{file} is not a file",
            usage::TOKEN_HEAD
        )));
    }
    Ok(Ask::TokenSet {
        app: w.app,
        file,
        json,
    })
}

fn promote(ws: &[&str]) -> Result<Ask, Stop> {
    let w = words(
        ws,
        &["--from", "--to", "--version"],
        &["--drop-platform", "--json"],
        unknown_flag,
        usage::PROMOTE_TWO,
    )?;
    let [from, to, ver] = [&w.values[0], &w.values[1], &w.values[2]];
    if w.app.is_empty() || to.is_empty() || (from.is_empty() && ver.is_empty()) {
        return Err(Stop::usage(usage::PROMOTE));
    }
    let body = Cj::new()
        .s("app", &w.app)
        .s("from", from)
        .s("to", to)
        .s("version", ver)
        .b("dropPlatforms", if w.switches[0] { "1" } else { "0" })
        .body();
    Ok(Ask::Promote {
        body,
        json: w.switches[1],
    })
}

fn rollback(ws: &[&str]) -> Result<Ask, Stop> {
    let w = words(
        ws,
        &["--channel", "--to"],
        &["--json"],
        unknown_flag,
        usage::ROLLBACK,
    )?;
    if w.app.is_empty() || w.values.iter().any(String::is_empty) {
        return Err(Stop::usage(usage::ROLLBACK));
    }
    let body = Cj::new()
        .s("app", &w.app)
        .s("channel", &w.values[0])
        .s("to", &w.values[1])
        .body();
    Ok(Ask::Rollback {
        body,
        json: w.switches[0],
    })
}

fn pull(ws: &[&str]) -> Result<Ask, Stop> {
    let w = words(ws, &["--version"], &["--json"], unknown_flag, usage::PULL)?;
    if w.app.is_empty() || w.values[0].is_empty() {
        return Err(Stop::usage(usage::PULL));
    }
    let body = Cj::new().s("app", &w.app).s("version", &w.values[0]).body();
    Ok(Ask::Pull {
        body,
        json: w.switches[0],
    })
}

/// `_app_updates_abs`: a relative path under `$(pwd -P)`.
fn abs(f: &str) -> String {
    if f.starts_with('/') {
        return f.to_string();
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    format!("{}/{f}", cwd.display())
}

/// `p="${a%%=*}"; f="${a#*=}"` and its check.
fn platform_file<'a>(a: &'a str, flag: &str, shape: &str) -> Result<(&'a str, &'a str), Stop> {
    match a.split_once('=') {
        Some((p, f)) if !p.is_empty() && !f.is_empty() => Ok((p, f)),
        _ => Err(Stop::usage(format!(
            "{}{flag} takes <platform>={shape} (got '{a}')",
            usage::PUSH_HEAD
        ))),
    }
}

fn push(ws: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut ch, mut ver, mut notes_file, mut evidence) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let (mut wait, mut json) = (true, false);
    let (mut arts, mut sigs, mut drops): (Vec<String>, Vec<String>, Vec<String>) =
        (Vec::new(), Vec::new(), Vec::new());
    let mut rest = ws;
    while let Some(w) = rest.first().copied() {
        let given = rest.get(1).copied().unwrap_or("").to_string();
        let took = match w {
            "--channel" => {
                ch = given;
                true
            }
            "--version" => {
                ver = given;
                true
            }
            "--artifact" => {
                arts.push(given);
                true
            }
            "--sig" => {
                sigs.push(given);
                true
            }
            "--notes-file" => {
                notes_file = given;
                true
            }
            "--drop-platform" => {
                drops.push(given);
                true
            }
            "--evidence" => {
                if given.is_empty() {
                    return Err(Stop::usage(usage::PUSH));
                }
                evidence = given;
                true
            }
            _ => false,
        };
        if took {
            if rest.len() < 2 {
                return Err(Stop::need_value(w));
            }
            rest = &rest[2..];
            continue;
        }
        match w {
            "--no-wait" => wait = false,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ if app.is_empty() => app = w.to_string(),
            _ => return Err(Stop::usage(usage::PUSH)),
        }
        rest = &rest[1..];
    }
    if app.is_empty() || ch.is_empty() || ver.is_empty() || arts.is_empty() {
        return Err(Stop::usage(usage::PUSH));
    }
    let mut list = Vec::new();
    for a in &arts {
        let (p, f) = platform_file(a, "--artifact", "<file>")?;
        if !std::path::Path::new(f).is_file() {
            return Err(Stop::usage(format!(
                "{}{f} is not a file",
                usage::PUSH_HEAD
            )));
        }
        list.push((p.to_string(), abs(f)));
    }
    let mut siglist = Vec::new();
    for a in &sigs {
        let (p, f) = platform_file(a, "--sig", "<file.sig>")?;
        siglist.push((p.to_string(), abs(f)));
    }
    let mut notes = String::new();
    if !notes_file.is_empty() {
        if !std::path::Path::new(&notes_file).is_file() {
            return Err(Stop::usage(format!(
                "{}{notes_file} is not a file",
                usage::PUSH_HEAD
            )));
        }
        notes = cat(&notes_file).map_err(|line| Stop {
            line,
            exit: Exit::Error,
        })?;
    }
    // The Python: `dict(l.split("\t", 1) …)` keeps the last sig of a
    // platform in the first one's place; each artifact takes its sig.
    let mut sig_map: Vec<(String, String)> = Vec::new();
    for (p, f) in siglist {
        match sig_map.iter_mut().find(|(k, _)| *k == p) {
            Some(slot) => slot.1 = f,
            None => sig_map.push((p, f)),
        }
    }
    let mut artifacts = Vec::new();
    for (p, f) in list {
        let mut a = vec![
            (Text::from("platform"), Value::Str(Text::from(p.as_str()))),
            (Text::from("path"), Value::Str(Text::from(f.as_str()))),
        ];
        if let Some(i) = sig_map.iter().position(|(k, _)| *k == p) {
            let (_, s) = sig_map.remove(i);
            a.push((Text::from("sig"), Value::Str(Text::from(s.as_str()))));
        }
        artifacts.push(Value::Object(a));
    }
    if !sig_map.is_empty() {
        let left: Vec<&str> = sig_map.iter().map(|(k, _)| k.as_str()).collect();
        return Err(Stop::usage(format!(
            "{}--sig for a platform with no --artifact: {}",
            usage::PUSH_HEAD,
            left.join(", ")
        )));
    }
    let drops: Vec<Value> = drops
        .iter()
        .flat_map(|d| pylib::ops::splitlines(d))
        .filter(|d| !d.is_empty())
        .map(|d| Value::Str(Text::from(d.as_str())))
        .collect();
    let mut b = vec![
        (Text::from("app"), Value::Str(Text::from(app.as_str()))),
        (Text::from("channel"), Value::Str(Text::from(ch.as_str()))),
        (Text::from("version"), Value::Str(Text::from(ver.as_str()))),
        (Text::from("artifacts"), Value::Array(artifacts)),
        (Text::from("dropPlatforms"), Value::Array(drops)),
    ];
    if !notes.is_empty() {
        b.push((Text::from("notes"), Value::Str(Text::from(notes.as_str()))));
    }
    if !evidence.is_empty() {
        b.push((
            Text::from("evidence"),
            Value::Str(Text::from(evidence.as_str())),
        ));
    }
    let body = json::to_string(&Value::Object(b), &Options::default()).unwrap_or_default();
    Ok(Ask::Push(Push {
        app,
        channel: ch,
        version: ver,
        body,
        wait,
        json,
    }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let answer = super::access::answer;
    match ask {
        Ask::KeyRead { app, json } => {
            let body = ctx.call(
                false,
                "/cli/app/updates/key",
                &format!("app={}", quote(app)),
            )?;
            Ok(answer(&body, *json, key_lines))
        }
        Ask::KeySet { body, json, .. } => {
            let s = ctx.send(true, "/cli/app/updates/key/set", body, false)?;
            if s.status == "202" {
                if *json {
                    let mut shown = super::wallet::raw(&s.body);
                    shown.exit = Exit::Usage;
                    return Ok(shown);
                }
                let mut o = Out::default();
                let r = held_lines(&s.body, &mut o);
                let mut shown = o.done(r);
                // A held step exits 2 by itself; one that isn't held,
                // `return 2` after it.
                if shown.exit == Exit::Ok {
                    shown.exit = Exit::Usage;
                }
                return Ok(shown);
            }
            let body = ok_200(ctx, s)?;
            Ok(answer(&body, *json, key_set_lines))
        }
        Ask::TokenRead { app, json } => {
            let body = ctx.call(
                false,
                "/cli/app/updates/token",
                &format!("app={}", quote(app)),
            )?;
            Ok(answer(&body, *json, token_lines))
        }
        Ask::TokenSet { app, file, json } => {
            let raw = if file.is_empty() {
                crate::prompt::read_all_stdin()
            } else {
                match std::fs::read(file) {
                    Ok(b) => crate::prompt::Secret::new(b),
                    Err(_) => return Err(Exit::Error),
                }
            };
            let text = String::from_utf8_lossy(raw.expose());
            let body = json::to_string(
                &Value::Object(vec![
                    (Text::from("app"), Value::Str(Text::from(app.as_str()))),
                    (
                        Text::from("token"),
                        Value::Str(Text::from(text.trim_matches(py_space))),
                    ),
                ]),
                &Options::default(),
            )
            .unwrap_or_default();
            drop(raw);
            let s = ctx.send(true, "/cli/app/updates/token/set", &body, false)?;
            drop(body);
            let body = ok_200(ctx, s)?;
            Ok(answer(&body, *json, token_set_lines))
        }
        Ask::Promote { body, json } => {
            let s = ctx.send(true, "/cli/app/updates/promote", body, false)?;
            Ok(answer(&ok_200(ctx, s)?, *json, promote_lines))
        }
        Ask::Rollback { body, json } => {
            let s = ctx.send(true, "/cli/app/updates/rollback", body, false)?;
            Ok(answer(&ok_200(ctx, s)?, *json, rollback_lines))
        }
        Ask::Pull { body, json } => {
            let s = ctx.send(true, "/cli/app/updates/pull", body, false)?;
            Ok(answer(&ok_200(ctx, s)?, *json, pull_lines))
        }
        Ask::Push(p) => run_push(ctx, p),
    }
}

/// `[ "$_CP_STATUS" = "200" ] || _compute_fail …`.
fn ok_200(ctx: &Ctx, s: Split) -> Result<Vec<u8>, Exit> {
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

/// `_app_updates_push` after its checks: the POST (curl's 900 seconds),
/// then, unless `--no-wait`, a read every 2 seconds while the release
/// uploads.
fn run_push(ctx: &mut Ctx, p: &Push) -> Answer {
    let before = ctx.client.compute_post_max_time;
    ctx.client.compute_post_max_time = Some(Duration::from_secs(900));
    let s = ctx.send(true, "/cli/app/updates/push", &p.body, false);
    ctx.client.compute_post_max_time = before;
    let body = ok_200(ctx, s?)?;
    let mut o = Out::default();
    let rid = match (|| {
        let v = py::load(&body, 1)?;
        Ok(py::s(&py::idx(&v, "release", 1)?))
    })() {
        Ok(r) => r.trim_end_matches('\n').to_string(),
        Err(e) => return Ok(o.done(Err(e))),
    };
    if !p.wait {
        if p.json {
            o.line(&String::from_utf8_lossy(&body));
        } else {
            o.line(&format!(
                "Uploading {} {} ({rid}) to the download host. Follow it: {} app updates ls {} --release {rid}",
                p.app,
                p.version,
                cli_name!(),
                p.app
            ));
        }
        return Ok(o.done(Ok(Exit::Usage)));
    }
    if !p.json {
        // Printed now: the wait can be long.
        let _ = crate::out::stdout(&format!(
            "Checked every signature; uploading {} {} ({rid}) to the download host\u{2026}\n",
            p.app, p.version
        ));
    }
    let q = format!("app={}&release={}", quote(&p.app), quote(&rid));
    let (mut state, mut waited, mut last) = ("uploading".to_string(), 0u64, Vec::new());
    while state == "uploading" {
        std::thread::sleep(Duration::from_secs(2));
        waited += 2;
        last = ctx.call(false, "/cli/app/updates", &q)?;
        state = match release_state(&last) {
            Ok(s) => s,
            Err(e) => return Ok(o.done(Err(e))),
        };
        if state == "uploading" && waited % 60 == 0 && !p.json {
            let _ = crate::out::stdout(&format!("  still uploading ({waited}s)\u{2026}\n"));
        }
    }
    if p.json {
        o.line(&String::from_utf8_lossy(&last));
    } else if let Err(e) = release_lines(&last, &rid, &mut o) {
        return Ok(o.done(Err(e)));
    }
    if state == "live" {
        if !p.json {
            o.line(&format!("{} now serves {}.", p.channel, p.version));
        }
        return Ok(o.done(Ok(Exit::Ok)));
    }
    Ok(o.done(Ok(Exit::Error)))
}

/// The state line of the wait's Python.
fn release_state(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 3)?;
    let r = py::or(py::dget(&v, "releases", 4)?, py::empty_list());
    let s = if py::truthy(&r) {
        let first = match &r {
            Value::Array(items) => items[0].clone(),
            other => py::head(other, 1, 5)?,
        };
        py::s(&py::idx(&first, "state", 5)?)
    } else {
        "missing".to_string()
    };
    Ok(s.trim_end_matches('\n').to_string())
}

/// `_app_updates_print_release`'s Python.
fn release_lines(body: &[u8], rid: &str, o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let rid_v = py::text(rid);
    let releases = py::dget_or(&v, "releases", py::empty_list(), 5)?;
    for r in py::iter(&releases, 5)? {
        if !pylib::ops::py_eq(&py::idx(&r, "id", 6)?, &rid_v)
            && !pylib::ops::py_eq(&py::idx(&r, "version", 6)?, &rid_v)
        {
            continue;
        }
        let chans = py::or(py::dget(&r, "channels", 8)?, py::empty_list());
        let on = py::join(", ", &chans, 8)?;
        let on = if on.is_empty() {
            "no channel".to_string()
        } else {
            on
        };
        o.line(&format!(
            "{} {}: {} ({on})",
            py::s(&py::idx(&v, "app", 9)?),
            py::s(&py::idx(&r, "version", 9)?),
            py::s(&py::idx(&r, "state", 9)?)
        ));
        if py::truthy(&py::dget(&r, "error", 10)?) {
            o.line(&format!("  error: {}", py::s(&py::idx(&r, "error", 11)?)));
        }
        let files = py::dget_or(&r, "files", py::empty_list(), 12)?;
        for f in py::iter(&files, 12)? {
            let platform = py::idx(&f, "platform", 13)?;
            let mib = py::div(&py::idx(&f, "size", 13)?, 1_048_576.0, 13)?;
            let size = format!("{} MiB", py::f(mib, 0, 1));
            let url = py::or(py::dget(&f, "url", 13)?, py::dget(&f, "upload", 13)?);
            o.line(&format!(
                "  {} {}  {}",
                py::left(&platform, 24),
                py::rjust(&size, 10),
                py::s(&url)
            ));
        }
    }
    Ok(())
}

/// `_app_held_exit`'s Python: the held step's lines, exit 2; done, 0;
/// failed, 3 with its line on stderr.
pub fn held_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let dict_or_empty = |x: Value| match x {
        Value::Object(_) => x,
        _ => Value::Object(Vec::new()),
    };
    if py::dget(&v, "status", 4)? == py::text("failed") {
        let r = dict_or_empty(py::dget(&v, "result", 5)?);
        let mut why = py::dget(&r, "hint", 6)?;
        if !py::truthy(&why) {
            why = py::or(py::dget(&r, "code", 6)?, py::text("applying it failed"));
        }
        o.err(&format!("not applied: {}", py::s(&why)));
        return Ok(Exit::Refused);
    }
    if py::dget(&v, "status", 8)? != py::text("held") {
        let mut new = py::dget(&v, "added", 9)?;
        if !py::truthy(&new) {
            let svcs = py::or(py::dget(&v, "services", 9)?, py::empty_list());
            let mut names = Vec::new();
            for x in py::iter(&svcs, 9)? {
                if let Value::Object(_) = x {
                    let n = py::dget(&x, "name", 9)?;
                    if py::truthy(&n) {
                        names.push(n);
                    }
                }
            }
            new = Value::Array(names);
        }
        let already = py::or(py::dget(&v, "alreadyAllowed", 10)?, py::empty_list());
        if py::truthy(&new) {
            o.line(&format!("allowed: {}", py::join(", ", &new, 12)?));
        }
        if py::truthy(&already) {
            o.line(&format!(
                "already allowed: {}",
                py::join(", ", &already, 14)?
            ));
        }
        let r = dict_or_empty(py::dget(&v, "result", 15)?);
        let mut note = py::dget(&r, "note", 16)?;
        if !py::truthy(&note) {
            note = if py::truthy(&new) || !py::truthy(&already) {
                py::dget(&v, "hint", 16)?
            } else {
                Value::Null
            };
        }
        if py::truthy(&note) {
            o.line(&py::concat("  ", &note, 18)?);
        }
        return Ok(Exit::Ok);
    }
    let r = py::idx(&v, "request", 20)?;
    let id = py::idx(&r, "id", 21)?;
    o.line(&format!("held for the owner: {}", py::s(&id)));
    o.line(&format!("  {}", py::s(&py::idx(&r, "summary", 22)?)));
    if py::truthy(&py::dget(&v, "alreadyAllowed", 23)?) {
        let a = py::idx(&v, "alreadyAllowed", 24)?;
        o.line(&format!("  already allowed: {}", py::join(", ", &a, 24)?));
    }
    if py::truthy(&py::dget(&v, "reused", 25)?) {
        let ago = py::or(py::dget(&v, "askedAgo", 26)?, py::text("a while"));
        o.line(&format!(
            "  this is your earlier request {} (asked {} ago), still pending; nothing new was filed",
            py::s(&py::idx(&r, "id", 26)?),
            py::s(&ago)
        ));
    }
    let c = dict_or_empty(py::dget(&v, "card", 27)?);
    if py::truthy(&py::dget(&c, "where", 28)?) {
        o.line(&format!("  owner: {}", py::s(&py::idx(&c, "where", 29)?)));
    }
    let hint = py::dget_or(&v, "hint", py::text(""), 30)?;
    o.line(&py::concat("  ", &hint, 30)?);
    Ok(Exit::Usage)
}

fn key_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if !py::truthy(&py::dget(&v, "pubkey", 4)?) {
        let app = py::s(&py::idx(&v, "app", 5)?);
        o.line(&format!(
            "{app} has no updater key: {} app updates key {app} --set <pubkey>",
            cli_name!()
        ));
    } else {
        o.line(&format!(
            "{} updater key {} (set by {})",
            py::s(&py::idx(&v, "app", 7)?),
            py::s(&py::idx(&v, "keyId", 7)?),
            py::s(&py::dget(&v, "setBy", 7)?)
        ));
        o.line(&py::s(&py::idx(&v, "pubkey", 8)?));
    }
    Ok(Exit::Ok)
}

fn key_set_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "unchanged", 4)?) {
        o.line(&format!(
            "{} already has updater key {}",
            py::s(&py::idx(&v, "app", 5)?),
            py::s(&py::idx(&v, "keyId", 5)?)
        ));
    } else {
        let app = py::s(&py::idx(&v, "app", 7)?);
        let kid = py::s(&py::idx(&v, "keyId", 7)?);
        let how = if py::truthy(&py::dget(&v, "changed", 7)?) {
            "changed"
        } else {
            "set"
        };
        o.line(&format!("{app} updater key {kid} {how}"));
    }
    Ok(Exit::Ok)
}

fn token_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "set", 4)?) {
        o.line(&format!(
            "{}: download-host token set (fingerprint {}) for {} on {}",
            py::s(&py::idx(&v, "app", 5)?),
            py::s(&py::idx(&v, "fingerprint", 5)?),
            py::s(&py::dget(&v, "dlId", 5)?),
            py::s(&py::dget(&v, "host", 5)?)
        ));
    } else {
        let app = py::s(&py::idx(&v, "app", 7)?);
        let dl = py::s(&py::dget(&v, "dlId", 7)?);
        let app2 = py::s(&py::idx(&v, "app", 7)?);
        o.line(&format!(
            concat!(
                "{}: no download-host token. Ask ",
                product_name!(),
                " Web for one for {}, then ",
                cli_name!(),
                " app updates token {} --file <file>"
            ),
            app, dl, app2
        ));
    }
    Ok(Exit::Ok)
}

fn token_set_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    o.line(&format!(
        "{}: download-host token kept in the vault (fingerprint {}, id {})",
        py::s(&py::idx(&v, "app", 4)?),
        py::s(&py::idx(&v, "fingerprint", 4)?),
        py::s(&py::idx(&v, "dlId", 4)?)
    ));
    Ok(Exit::Ok)
}

fn promote_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "unchanged", 4)?) {
        o.line(&format!(
            "{} {} already serves {}",
            py::s(&py::idx(&v, "app", 5)?),
            py::s(&py::idx(&v, "channel", 5)?),
            py::s(&py::idx(&v, "version", 5)?)
        ));
    } else {
        o.line(&format!(
            "{} {} now serves {} (no re-upload). Fallback latest.json: {}",
            py::s(&py::idx(&v, "app", 7)?),
            py::s(&py::idx(&v, "channel", 7)?),
            py::s(&py::idx(&v, "version", 7)?),
            py::s(&py::dget(&v, "fallback", 7)?)
        ));
    }
    Ok(Exit::Ok)
}

fn rollback_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    o.line(&format!(
        "{} {}: {} -> {}",
        py::s(&py::idx(&v, "app", 4)?),
        py::s(&py::idx(&v, "channel", 4)?),
        py::s(&py::idx(&v, "from", 4)?),
        py::s(&py::idx(&v, "version", 4)?)
    ));
    o.line(&py::concat("WARNING: ", &py::idx(&v, "warning", 5)?, 5)?);
    Ok(Exit::Ok)
}

fn pull_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "unchanged", 4)?) {
        o.line(&format!(
            "{} {} was already pulled",
            py::s(&py::idx(&v, "app", 5)?),
            py::s(&py::idx(&v, "version", 5)?)
        ));
        return Ok(Exit::Ok);
    }
    o.line(&format!(
        "{} {} pulled.",
        py::s(&py::idx(&v, "app", 6)?),
        py::s(&py::idx(&v, "version", 6)?)
    ));
    let chans = py::dget_or(&v, "channels", py::empty_list(), 7)?;
    for c in py::iter(&chans, 7)? {
        let now = py::or(py::dget(&c, "now", 8)?, py::text("nothing (empty)"));
        o.line(&format!(
            "  {} now serves {}",
            py::s(&py::idx(&c, "channel", 8)?),
            py::s(&now)
        ));
    }
    o.line(&py::s(&py::dget_or(&v, "note", py::text(""), 9)?));
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        assert_eq!(
            parse(&["key", "acme"]),
            Some(Ok(Ask::KeyRead {
                app: "acme".into(),
                json: false
            }))
        );
        assert_eq!(
            parse(&["key", "acme", "--set"]),
            Some(Err(Stop::need_value("--set")))
        );
        assert_eq!(
            parse(&["key", "acme", "--set", ""]),
            Some(Err(Stop::usage(format!(
                "{}the key is empty",
                usage::KEY_HEAD
            ))))
        );
        assert_eq!(
            parse(&["key", "a", "b"]),
            Some(Err(Stop::usage(usage::KEY)))
        );
        assert_eq!(
            parse(&["token", "a", "--x"])
                .and_then(Result::err)
                .map(|s| s.line),
            Some(format!("Unknown flag: --x{}", usage::TOKEN_FLAG_TAIL))
        );
        assert_eq!(
            parse(&["promote", "a", "--to", "beta"]),
            Some(Err(Stop::usage(usage::PROMOTE)))
        );
        assert!(matches!(
            parse(&["promote", "a", "--to", "beta", "--version", "1.2"]),
            Some(Ok(Ask::Promote { body, .. })) if body == r#"{"app": "a", "to": "beta", "version": "1.2", "dropPlatforms": false}"#
        ));
        assert_eq!(
            parse(&["push", "a", "--evidence"]),
            Some(Err(Stop::usage(usage::PUSH)))
        );
        assert_eq!(parse(&["ls", "a"]), None);
        assert_eq!(parse(&["pull", "a", "-h"]), None);
    }
}
