//! Box usage and the disk guard, as the shell CLI answers them
//! (`cmd_app_resources`, `cli/src/app-grace.sh`): `app resources [--app
//! A] [--owner AGENT] [--rescan [--wait]]`, `app resources settings`,
//! `app resources clean` and `app resources trends`. Each is the shell's
//! word loop, its requests, `--json` as it came, else the shell's Python
//! line for line. The view's printer is [`super::resources`]' (the one
//! `app inspect --resources` uses).
//!
//! `--rescan --wait` asks the view every 5 seconds, at most 120 times,
//! until the scan is no longer running, as the shell's loop does.
//!
//! The shell's: a size for `settings` with a character outside ASCII
//! (Python's `\d` and `\s` take more than the binary reads).

use super::call::{Cj, Ctx};
use super::grace_set::{help, walk};
use super::py::{self, Out};
use super::resources::{float, gb};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py as pyfmt;
use crate::out::pylib::ops::{py_eq, round_int};
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const VIEW: &str = concat!(
        "usage: ",
        cli_name!(),
        " app resources [--json] [--app A] [--owner AGENT] [--rescan [--wait]]"
    );
    pub const WAIT: &str = "--wait goes with --rescan";
    pub const SETTINGS_TAIL: &str = concat!(" (", cli_name!(), " app resources --help)");
    pub const MODE: &str = "--mode is on, warn or off";
    pub const CLEAN: &str = concat!(
        "usage: ",
        cli_name!(),
        " app resources clean [--dry-run] [--json]"
    );
    pub const TRENDS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app resources trends [--app A] [--window 1h|6h|24h|7d|30d] [--json]"
    );
}

/// `"No samples yet: K2 samples once a minute and keeps 24 h of minutes
/// and 30 days of hours."`.
const NO_SAMPLES: &str = concat!(
    "No samples yet: ",
    product_name!(),
    " samples once a minute and keeps 24 h of minutes and 30 days of hours."
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    View {
        query: String,
        rescan: bool,
        wait: bool,
        json: bool,
    },
    /// `body`: the POST's, when a flag asked for a change.
    Settings {
        body: Option<String>,
        json: bool,
    },
    Clean {
        dry: bool,
        json: bool,
    },
    Trends {
        query: String,
        json: bool,
    },
}

/// The words after `resources`. `None`: a help form, or a size the shell
/// reads (see the module's notes).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    let rest = words.get(1..).unwrap_or_default();
    Some(match words.first().copied() {
        Some("settings") => return settings(rest),
        Some("clean") => clean(rest),
        Some("trends") => trends(rest),
        _ => view(words),
    })
}

fn view(words: &[&str]) -> Result<Ask, Stop> {
    let w = loop_words(
        words,
        &["--app", "--owner"],
        &["--json", "--rescan", "--wait"],
        |w: &str| {
            if w.starts_with('-') {
                format!("Unknown flag: {w}")
            } else {
                usage::VIEW.to_string()
            }
        },
    )?;
    let (rescan, wait) = (w.has("--rescan"), w.has("--wait"));
    if wait && !rescan {
        return Err(Stop::usage(usage::WAIT));
    }
    let mut query = String::new();
    let (app, owner) = (w.v("--app"), w.v("--owner"));
    if !app.is_empty() {
        query = format!("app={}", quote(&app));
    }
    if !owner.is_empty() {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&format!("owner={}", quote(&owner)));
    }
    Ok(Ask::View {
        query,
        rescan,
        wait,
        json: w.has("--json"),
    })
}

/// The shell's loop of value flags and switches, in order: a word that is
/// neither ends it with `other`'s line (exit 2); a value flag given last
/// is `_need_value`'s line (exit 2).
fn loop_words(
    words: &[&str],
    values: &[&'static str],
    switches: &[&'static str],
    other: impl Fn(&str) -> String,
) -> Result<super::grace_set::Words, Stop> {
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        if values.contains(&w) {
            if rest.len() < 2 {
                return Err(Stop::need_value(w));
            }
            rest = &rest[2..];
            continue;
        }
        if !switches.contains(&w) {
            return Err(Stop::usage(other(w)));
        }
        rest = &rest[1..];
    }
    let names: Vec<&[&'static str]> = values.iter().map(std::slice::from_ref).collect();
    walk(words, &names, switches)
}

/// `--warn-free 20G` and the like: the bytes, or `-` for none; `Err` the
/// shell's stop; `None` the shell's.
fn size_bytes(s: &str) -> Option<Result<String, Stop>> {
    if !s.is_ascii() {
        return None;
    }
    let t = s.trim_matches(|c: char| c.is_ascii_whitespace() || ('\x1c'..='\x1f').contains(&c));
    if t.is_empty() {
        return Some(Ok(String::new()));
    }
    let bad = || {
        Some(Err(Stop::usage(format!(
            "bad size {} (use 20G, 512M or bytes)",
            pyfmt::repr(&py::text(t))
        ))))
    };
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return bad();
    }
    let mut end = digits;
    let b = t.as_bytes();
    if b.get(end) == Some(&b'.') {
        let frac = b[end + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if frac == 0 {
            return bad();
        }
        end += 1 + frac;
    }
    let number = &t[..end];
    let mut rest = t[end..]
        .trim_start_matches(|c: char| c.is_ascii_whitespace() || ('\x1c'..='\x1f').contains(&c));
    let mult: f64 = match rest.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some(c @ ('k' | 'm' | 'g' | 't')) => {
            rest = &rest[1..];
            match c {
                'k' => 1024.0,
                'm' => 1_048_576.0,
                'g' => 1_073_741_824.0,
                _ => 1_099_511_627_776.0,
            }
        }
        _ => 1.0,
    };
    if !matches!(rest, "" | "b" | "B" | "ib" | "iB") {
        return bad();
    }
    let x: f64 = number.parse().ok()?;
    let r = round_int(x * mult, 0).ok()?;
    Some(Ok(r))
}

fn settings(words: &[&str]) -> Option<Result<Ask, Stop>> {
    const VALUES: [&str; 7] = [
        "--mode",
        "--warn",
        "--warn-free",
        "--refuse",
        "--reserve",
        "--critical",
        "--critical-free",
    ];
    let w = match loop_words(words, &VALUES, &["--json"], |w: &str| {
        format!("Unknown argument: {w}{}", usage::SETTINGS_TAIL)
    }) {
        Ok(w) => w,
        Err(stop) => return Some(Err(stop)),
    };
    let changed = VALUES.iter().any(|f| w.given(f));
    let json = w.has("--json");
    let mode = w.v("--mode");
    if !matches!(mode.as_str(), "" | "on" | "warn" | "off") {
        return Some(Err(Stop::usage(usage::MODE)));
    }
    let (warn, refuse, critical) = (w.v("--warn"), w.v("--refuse"), w.v("--critical"));
    for p in [&warn, &refuse, &critical] {
        if !(p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit())) {
            return Some(Err(Stop::usage(format!(
                "percent lines are whole numbers 1-99 (got {p})"
            ))));
        }
    }
    let mut sizes = Vec::new();
    for f in ["--warn-free", "--reserve", "--critical-free"] {
        match size_bytes(&w.v(f))? {
            Ok(n) => sizes.push(n),
            Err(stop) => return Some(Err(stop)),
        }
    }
    if !changed {
        return Some(Ok(Ask::Settings { body: None, json }));
    }
    let body = Cj::new()
        .s("mode", &mode)
        .n("warnPct", &warn)
        .n("warnFreeBytes", &sizes[0])
        .n("refusePct", &refuse)
        .n("reserveBytes", &sizes[1])
        .n("criticalPct", &critical)
        .n("criticalFreeBytes", &sizes[2])
        .body();
    Some(Ok(Ask::Settings {
        body: Some(body),
        json,
    }))
}

fn clean(words: &[&str]) -> Result<Ask, Stop> {
    let (mut dry, mut json) = (false, false);
    for w in words {
        match *w {
            "--dry-run" => dry = true,
            "--json" => json = true,
            _ => return Err(Stop::usage(usage::CLEAN)),
        }
    }
    Ok(Ask::Clean { dry, json })
}

fn trends(words: &[&str]) -> Result<Ask, Stop> {
    let w = loop_words(words, &["--app", "--window"], &["--json"], |_: &str| {
        usage::TRENDS.to_string()
    })?;
    let mut query = String::new();
    let (app, window) = (w.v("--app"), w.v("--window"));
    if !app.is_empty() {
        query = format!("app={}", quote(&app));
    }
    if !window.is_empty() {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&format!("window={}", quote(&window)));
    }
    Ok(Ask::Trends {
        query,
        json: w.has("--json"),
    })
}

fn printed(body: &[u8], json: bool, lines: fn(&[u8], &mut Out) -> Result<(), Raise>) -> Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = lines(body, &mut o).map(|()| Exit::Ok);
    o.done(r)
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::View {
            query,
            rescan,
            wait,
            json,
        } => {
            if *rescan {
                rescan_run(ctx, *wait)?;
            }
            let body = ctx.call(false, "/cli/app/resources", query)?;
            Ok(printed(&body, *json, super::resources::lines))
        }
        Ask::Settings { body, json } => {
            let answer = match body {
                Some(b) => {
                    let s = ctx.send(true, "/cli/app/resources/settings", b, false)?;
                    if !s.ok() {
                        return Err(ctx.fail(&s));
                    }
                    s.body
                }
                None => {
                    let got = ctx.call(false, "/cli/app/resources", "")?;
                    match guard_only(&got) {
                        Ok(text) => text.into_bytes(),
                        Err(e) => return Ok(raised(String::new(), String::new(), &e)),
                    }
                }
            };
            Ok(printed(&answer, *json, settings_lines))
        }
        Ask::Clean { dry, json } => {
            let b = Cj::new().b("dryRun", if *dry { "1" } else { "0" }).body();
            let s = ctx.post_max("/cli/app/resources/clean", &b, 1800)?;
            if !s.ok() {
                return Err(ctx.fail(&s));
            }
            Ok(printed(&s.body, *json, clean_lines))
        }
        Ask::Trends { query, json } => {
            let body = ctx.call(false, "/cli/app/resources/trends", query)?;
            Ok(printed(&body, *json, trends_lines))
        }
    }
}

/// `--rescan` (and `--wait`): its line on stderr, and the polls.
fn rescan_run(ctx: &mut Ctx, mut wait: bool) -> Result<(), Exit> {
    let body = ctx.call(true, "/cli/app/resources/rescan", "{}")?;
    let started = match started_word(&body) {
        Ok(w) => w,
        Err(e) => {
            let shown = raised(String::new(), String::new(), &e);
            return Err(shown.print());
        }
    };
    match started.as_str() {
        "started" | "running" => out::stderr_line(&format!("box usage scan: {started}")),
        "noscanner" => {
            out::stderr_line(
                "box usage scan: no scanner runs on this daemon yet; the numbers below are the disks only",
            );
            wait = false;
        }
        _ => {
            out::stderr_line(&format!(
                "box usage scan {started} (one every 5 minutes at most)"
            ));
            wait = false;
        }
    }
    if !wait {
        return Ok(());
    }
    let mut running = true;
    for _ in 0..120 {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let body = ctx.call(false, "/cli/app/resources", "")?;
        running = match still_running(&body) {
            Ok(r) => r,
            Err(e) => {
                let shown = raised(String::new(), String::new(), &e);
                return Err(shown.print());
            }
        };
        if !running {
            break;
        }
    }
    if running {
        out::stderr_line("the scan is still running after 10 minutes; showing what is known");
    }
    Ok(())
}

/// The rescan answer's word, as `$(…)` holds what the Python printed.
fn started_word(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 3)?;
    let w = if py::truthy(&py::dget(&v, "started", 4)?) {
        "started".to_string()
    } else if py::truthy(&py::dget(&v, "running", 6)?) {
        "running".to_string()
    } else if py::truthy(&py::dget(&v, "skipped", 8)?) {
        py::concat("skipped: ", &py::idx(&v, "skipped", 9)?, 9)?
    } else {
        "noscanner".to_string()
    };
    Ok(w.trim_end_matches('\n').to_string())
}

/// `print(1 if (json.load(sys.stdin).get("scan") or {}).get("running")
/// else 0)`.
fn still_running(body: &[u8]) -> Result<bool, Raise> {
    let v = py::load(body, 1)?;
    let scan = py::or(py::dget(&v, "scan", 1)?, Value::Object(Vec::new()));
    Ok(py::truthy(&py::dget(&scan, "running", 1)?))
}

/// `print(json.dumps({"ok": True, "guard": json.load(sys.stdin).get("guard")}))`,
/// as `$(…)` holds it.
fn guard_only(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 1)?;
    let guard = py::dget(&v, "guard", 1)?;
    let out = Value::Object(vec![
        (Text::from("ok"), Value::Bool(true)),
        (Text::from("guard"), guard),
    ]);
    Ok(json::to_string(&out, &Options::default()).unwrap_or_default())
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// The settings printer's `gib(b)`, called from `from`.
fn settings_gib(b: &Value, from: u32) -> Result<String, Raise> {
    if *b == Value::Null {
        return Ok("\u{2014}".into());
    }
    let x =
        py::div(b, 1_073_741_824.0, 9).map_err(|e| Raise::inside(from, 9, "gib", e.exception))?;
    round_int(x, 10).map_err(|e| Raise::inside(from, 10, "gib", e.exception))?;
    let places = if (x - x.round_ties_even()).abs() < 0.05 {
        0
    } else {
        1
    };
    Ok(format!("{} GiB", py::f(x, 0, places)))
}

/// `cmd_app_resources_settings`' Python.
fn settings_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let g = py::or(py::dget(&v, "guard", 4)?, empty_dict());
    let l = py::or(py::dget(&g, "lines", 5)?, empty_dict());
    if py::truthy(&py::dget(&v, "changed", 11)?) {
        o.line("disk guard saved");
    }
    o.line(&format!(
        "mode      {} ({})",
        py::s(&py::dget(&g, "mode", 13)?),
        py::s(&py::dget(&g, "modeSource", 13)?)
    ));
    let warn = py::dget(&l, "warnPct", 14)?;
    let wf = settings_gib(&py::dget(&l, "warnFreeBytes", 14)?, 14)?;
    o.line(&format!(
        "warn      {}% used or under {wf} free",
        py::s(&warn)
    ));
    let refuse = py::dget(&l, "refusePct", 15)?;
    let rs = settings_gib(&py::dget(&l, "reserveBytes", 15)?, 15)?;
    o.line(&format!(
        "refuse    {}% used, or an action that would leave under {rs}",
        py::s(&refuse)
    ));
    let crit = py::dget(&l, "criticalPct", 16)?;
    let cf = settings_gib(&py::dget(&l, "criticalFreeBytes", 16)?, 16)?;
    o.line(&format!(
        "critical  {}% used or under {cf} free",
        py::s(&crit)
    ));
    if py::truthy(&py::dget(&v, "note", 17)?) {
        o.line(&py::concat("note: ", &py::idx(&v, "note", 18)?, 18)?);
    }
    Ok(())
}

/// What a clean item's kind says.
const CLEAN_WORDS: [(&str, &str); 8] = [
    ("seed", concat!("unused Seed (", product_name!(), " copy)")),
    ("engineSeed", "unused Seed (walled copy)"),
    ("image", "unused image"),
    ("engineLeftover", "engine leftover"),
    ("rotatedLog", "rotated log"),
    ("deploymentLog", "old deployment log"),
    ("buildTemp", "build temp"),
    ("imageArchive", "old base image archive"),
];

/// `cmd_app_resources_clean`'s Python.
fn clean_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let dry = py::truthy(&py::dget(&v, "dryRun", 22)?);
    let items = py::or(py::dget(&v, "items", 23)?, py::empty_list());
    let scope = if py_eq(&py::dget(&v, "scope", 24)?, &py::text("box")) {
        "this box"
    } else {
        "your workspace's Apps and services"
    };
    o.line(&format!(
        "{} ({scope})",
        if dry { "Would free" } else { "Cleaned" }
    ));
    if py::truthy(&items) {
        o.line("");
        o.line(&format!(
            "{} {} {} ITEM",
            crate::families::ljust("KIND", 26),
            crate::families::ljust("SIZE", 9),
            crate::families::ljust("APP", 14)
        ));
    }
    for i in py::iter(&items, 28)? {
        let mut state = String::new();
        if py::truthy(&py::dget(&i, "error", 30)?) {
            state = format!("  (kept: {})", py::s(&py::idx(&i, "error", 31)?));
        } else if !dry && !py::truthy(&py::dget(&i, "removed", 32)?) {
            state = "  (kept)".into();
        }
        let kind = py::dget(&i, "kind", 34)?;
        let word = py::word_for(&CLEAN_WORDS, &kind, py::dget(&i, "kind", 34)?, 34)?;
        let size = gb(&py::dget(&i, "bytes", 34)?, 34)?;
        let app = py::or(py::dget(&i, "app", 34)?, py::text("\u{2014}"));
        let id = py::dget(&i, "id", 34)?;
        o.line(&format!(
            "{} {} {} {}{state}",
            py::left(&word, 26),
            crate::families::ljust(&size, 9),
            py::left(&app, 14),
            py::s(&id)
        ));
    }
    let t = py::or(py::dget(&v, "totals", 35)?, empty_dict());
    o.line("");
    if dry {
        o.line(&format!(
            "Total: about {}",
            gb(&py::dget(&t, "bytes", 38)?, 38)?
        ));
    } else {
        o.line(&format!(
            "Freed: about {}",
            gb(&py::dget(&t, "freedBytes", 40)?, 40)?
        ));
    }
    let e = py::or(py::dget(&v, "engine", 41)?, empty_dict());
    if py::truthy(&py::dget(&e, "note", 42)?) {
        o.line(&py::concat("Note: ", &py::idx(&e, "note", 43)?, 43)?);
    } else {
        let st = py::dget(&e, "status", 44)?;
        if let Value::Str(_) = st {
            let s = py::idx(&e, "status", 44)?;
            if py::s(&s).starts_with("error") {
                o.line(&py::concat(
                    "Note: the walled engine: ",
                    &py::idx(&e, "status", 45)?,
                    45,
                )?);
            }
        }
    }
    o.line(&py::s(&py::or(py::dget(&v, "never", 46)?, py::text(""))));
    if py::truthy(&py::dget(&v, "hint", 47)?) {
        o.line(&py::s(&py::idx(&v, "hint", 48)?));
    }
    Ok(())
}

const BARS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// A number Python adds and divides, or the `TypeError` at `line` of
/// `name` (called from `from`).
fn num(x: &Value, from: u32, line: u32, name: &'static str, op: &str) -> Result<f64, Raise> {
    match x {
        Value::Int(d) => Ok(d.parse::<f64>().unwrap_or(f64::INFINITY)),
        Value::Float(f) => Ok(*f),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        other => Err(Raise::inside(
            from,
            line,
            name,
            format!("TypeError: {op} '{}'", pyfmt::type_name(other)),
        )),
    }
}

/// `spark(vals, top)`, called from line `from`.
fn spark(vals: &[Value], top: &Value, from: u32) -> Result<String, Raise> {
    let known: Vec<&Value> = vals.iter().filter(|x| **x != Value::Null).collect();
    if known.is_empty() {
        return Ok(String::new());
    }
    let hi = if py::truthy(top) {
        num(
            top,
            from,
            17,
            "spark",
            "unsupported operand type(s) for /: 'float' and",
        )?
    } else {
        // `max(known) or 1`.
        let mut m = num(
            known[0],
            from,
            17,
            "spark",
            "'>' not supported between instances of 'int' and",
        )?;
        for x in &known[1..] {
            let y = num(
                x,
                from,
                17,
                "spark",
                "'>' not supported between instances of 'int' and",
            )?;
            if y > m {
                m = y;
            }
        }
        if m == 0.0 {
            1.0
        } else {
            m
        }
    };
    let n = vals.len();
    let step = n.div_ceil(40).max(1);
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let chunk: Vec<&Value> = vals[i..std::cmp::min(n, i + step)]
            .iter()
            .filter(|x| **x != Value::Null)
            .collect();
        i += step;
        if chunk.is_empty() {
            out.push(' ');
            continue;
        }
        let mut sum = 0.0;
        for x in &chunk {
            sum += num(
                x,
                from,
                27,
                "spark",
                "unsupported operand type(s) for +: 'int' and",
            )?;
        }
        let x = sum / chunk.len() as f64;
        let k = (x / hi * 7.0 + 0.5).trunc();
        if !k.is_finite() {
            return Err(Raise::inside(
                from,
                28,
                "spark",
                "OverflowError: cannot convert float infinity to integer",
            ));
        }
        let k = (k as i64).min(7);
        let idx = if k < 0 { 8 + k } else { k };
        if idx < 0 {
            return Err(Raise::inside(
                from,
                28,
                "spark",
                "IndexError: string index out of range",
            ));
        }
        out.push(BARS[idx as usize]);
    }
    Ok(out)
}

/// The trends printer's `gib(b)`.
fn trends_gib(b: &Value, from: u32) -> Result<String, Raise> {
    if *b == Value::Null {
        return Ok("\u{2014}".into());
    }
    let g = float(b, 8).map_err(|e| Raise::inside(from, 8, "gib", e.exception))?
        / f64::from(1u32 << 30);
    Ok(if g >= 10.0 {
        format!("{} GiB", py::f(g, 0, 0))
    } else {
        format!("{} GiB", py::f(g, 0, 1))
    })
}

/// `cores(c)`.
fn cores(c: &Value, from: u32) -> Result<String, Raise> {
    if *c == Value::Null {
        return Ok("\u{2014}".into());
    }
    let x = match c {
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY),
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        other => {
            return Err(Raise::inside(
                from,
                11,
                "cores",
                format!(
                    "TypeError: must be real number, not {}",
                    pyfmt::type_name(other)
                ),
            ))
        }
    };
    Ok(py::f(x, 0, 2))
}

/// `line(label, pts, summ, budget, total)`, called from `from`.
fn trend_line(
    o: &mut Out,
    label: &str,
    pts: &Value,
    summ: &Value,
    budget: &Value,
    total: &Value,
    from: u32,
) -> Result<(), Raise> {
    let inner = |line: u32, e: Raise| Raise::inside(from, line, "line", e.exception);
    let s = py::or(summ.clone(), empty_dict());
    let avg = py::dget(&s, "memAvgBytes", 32).map_err(|e| inner(32, e))?;
    let peak = py::dget(&s, "memPeakBytes", 32).map_err(|e| inner(32, e))?;
    let mut mem = format!(
        "mem avg {} peak {}",
        trends_gib(&avg, from)?,
        trends_gib(&peak, from)?
    );
    if py::truthy(budget) {
        mem.push_str(&format!(" of {} budget", trends_gib(budget, from)?));
    }
    let ca = py::dget(&s, "cpuAvgCores", 35).map_err(|e| inner(35, e))?;
    let cp = py::dget(&s, "cpuPeakCores", 35).map_err(|e| inner(35, e))?;
    o.line(&format!(
        "{} cpu avg {} peak {} cores \u{b7} {mem}",
        crate::families::ljust(label, 24),
        cores(&ca, from)?,
        cores(&cp, from)?
    ));
    let points = py::iter(pts, 36).map_err(|e| inner(36, e))?;
    let mut cpu = Vec::new();
    for p in &points {
        cpu.push(py::dget(p, "cpuCores", 36).map_err(|e| inner(36, e))?);
    }
    let c = spark(&cpu, &Value::Null, from)?;
    let mut memv = Vec::new();
    for p in &points {
        let mp = py::dget(p, "memPeakBytes", 37).map_err(|e| inner(37, e))?;
        memv.push(if mp != Value::Null {
            py::dget(p, "memPeakBytes", 37).map_err(|e| inner(37, e))?
        } else {
            py::dget(p, "memBytes", 37).map_err(|e| inner(37, e))?
        });
    }
    let top = py::or(budget.clone(), total.clone());
    let m = spark(&memv, &top, from)?;
    if !c.is_empty() {
        o.line(&format!("    cpu {c}"));
    }
    if !m.is_empty() {
        o.line(&format!("    mem {m}"));
    }
    Ok(())
}

/// `cmd_app_resources_trends`' Python.
fn trends_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let b = py::or(py::dget(&v, "box", 42)?, empty_dict());
    let every = if py_eq(&py::dget(&v, "resSecs", 43)?, &Value::Int("60".into())) {
        "minute"
    } else {
        "hour"
    };
    o.line(&format!(
        "CPU and memory, last {} (one point every {every})",
        py::s(&py::dget(&v, "window", 43)?)
    ));
    o.line("");
    let label = format!(
        "box ({} cores)",
        py::s(&py::or(py::dget(&b, "cores", 45)?, py::text("?")))
    );
    let pts = py::or(py::dget(&b, "points", 45)?, py::empty_list());
    let summ = py::dget(&b, "summary", 45)?;
    let total = py::dget(&b, "memTotalBytes", 45)?;
    trend_line(o, &label, &pts, &summ, &Value::Null, &total, 45)?;
    let apps = py::or(py::dget(&v, "apps", 46)?, py::empty_list());
    let svcs = py::or(py::dget(&v, "services", 47)?, py::empty_list());
    if py::truthy(&apps) || py::truthy(&svcs) {
        o.line("");
    }
    for a in py::iter(&apps, 50)? {
        let owner = py::or(py::dget(&a, "owner", 51)?, empty_dict());
        let who = py::or(py::dget(&owner, "handle", 51)?, py::text("server"));
        let label = format!("{} ({})", py::s(&py::dget(&a, "app", 52)?), py::s(&who));
        let pts = py::or(py::dget(&a, "points", 52)?, py::empty_list());
        let summ = py::dget(&a, "summary", 52)?;
        let budget = py::dget(&a, "memBudgetBytes", 52)?;
        trend_line(o, &label, &pts, &summ, &budget, &Value::Null, 52)?;
    }
    for s in py::iter(&svcs, 53)? {
        let owner = py::or(py::dget(&s, "owner", 54)?, empty_dict());
        let who = py::or(py::dget(&owner, "handle", 54)?, py::text("server"));
        let label = format!(
            "{} (--cmd, {})",
            py::s(&py::dget(&s, "service", 55)?),
            py::s(&who)
        );
        let pts = py::or(py::dget(&s, "points", 55)?, py::empty_list());
        let summ = py::dget(&s, "summary", 55)?;
        trend_line(o, &label, &pts, &summ, &Value::Null, &Value::Null, 55)?;
    }
    let any = py::truthy(&py::dget(&b, "points", 56)?) || py::truthy(&apps) || py::truthy(&svcs);
    if !any {
        o.line(NO_SAMPLES);
    }
    o.line("");
    o.line("RAM budgets are shown beside use; nothing here stops an App for using more. \u{2014} is not measured on this OS.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_the_python_reads_them() {
        assert_eq!(size_bytes("20G").unwrap().unwrap(), "21474836480");
        assert_eq!(size_bytes(" 1.5 MiB ").unwrap().unwrap(), "1572864");
        assert_eq!(size_bytes("512").unwrap().unwrap(), "512");
        assert_eq!(size_bytes("").unwrap().unwrap(), "");
        assert_eq!(
            size_bytes("20X").unwrap(),
            Err(Stop::usage("bad size '20X' (use 20G, 512M or bytes)"))
        );
        assert_eq!(
            size_bytes("1.G").unwrap(),
            Err(Stop::usage("bad size '1.G' (use 20G, 512M or bytes)"))
        );
        assert!(size_bytes("2\u{0663}").is_none());
    }

    #[test]
    fn a_spark_line_reads_as_the_python_draws_it() {
        let v = |x: i64| Value::Int(x.to_string());
        assert_eq!(
            spark(&[v(0), v(1), Value::Null, v(4)], &Value::Null, 36).unwrap(),
            "\u{2581}\u{2583} \u{2588}"
        );
        assert_eq!(spark(&[Value::Null], &Value::Null, 36).unwrap(), "");
    }

    #[test]
    fn the_loops_read_the_words_as_the_shell_does() {
        assert_eq!(parse(&["--wait"]), Some(Err(Stop::usage(usage::WAIT))));
        assert_eq!(parse(&["x"]), Some(Err(Stop::usage(usage::VIEW))));
        assert_eq!(
            parse(&["settings", "x"]),
            Some(Err(Stop::usage(format!(
                "Unknown argument: x{}",
                usage::SETTINGS_TAIL
            ))))
        );
        assert_eq!(
            parse(&["settings", "--warn", "100"]),
            Some(Err(Stop::usage(
                "percent lines are whole numbers 1-99 (got 100)"
            )))
        );
        assert_eq!(
            parse(&["trends", "--app"]),
            Some(Err(Stop::usage("Error: --app takes a value")))
        );
    }
}
