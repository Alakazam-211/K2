//! `app inspect <app-id> --resources [--json]`: the App's row of the box
//! usage view, as `cmd_app_inspect` asks for it (`cli/src/app-doctor.sh`:
//! one `_compute_call GET /cli/app/resources app=<app>`) and
//! `_app_resources_print` prints it (`cli/src/app-grace.sh`), line for
//! line: `--json` is the answer as it came; a step of the Python that
//! raises ends the command with its traceback and exit 1.
//!
//! The times read the clock as the Python does: `checked` is the box's
//! `checkedAt` (now when it has none) as local `%H:%M`, and `folders
//! scanned` is how long ago the last scan was.

use super::call::Ctx;
use super::catalog::plus;
use super::py::{self, Out};
use super::wallet::raw;
use super::Answer;
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::{Text, Value};
use crate::out::py as pyfmt;
use crate::out::pylib;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// `"Not measured by K2 (OS, other users)"`.
const NOT_MEASURED: &str = concat!("Not measured by ", product_name!(), " (OS, other users)");
/// `"Ask an agent:    k2 msg %s \"…\""`.
const ASK: &str = concat!("Ask an agent:    ", cli_name!(), " msg ");
/// `"K2 will clear:   "`.
const CLEAR: &str = concat!(product_name!(), " will clear:   ");

/// Runs it: what it printed, or the exit the request ended with.
pub fn run(ctx: &mut Ctx, app: &str, json: bool) -> Answer {
    let body = ctx.call(false, "/cli/app/resources", &format!("app={}", quote(app)))?;
    if json {
        return Ok(raw(&body));
    }
    let mut o = Out::default();
    let r = lines(&body, &mut o).map(|()| Exit::Ok);
    Ok(o.done(r))
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `x.get(key) or {}`.
fn dict_at(x: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    Ok(py::or(py::dget(x, key, line)?, empty_dict()))
}

/// `float(b)`.
pub(super) fn float(b: &Value, line: u32) -> Result<f64, Raise> {
    match b {
        Value::Float(f) => Ok(*f),
        Value::Bool(x) => Ok(f64::from(u8::from(*x))),
        Value::Int(digits) => {
            let f: f64 = digits.parse().unwrap_or(f64::INFINITY);
            if f.is_infinite() {
                return Err(Raise::at(
                    line,
                    "OverflowError: int too large to convert to float",
                ));
            }
            Ok(f)
        }
        Value::Str(t) => t.as_str().and_then(pylib::ops::float).ok_or_else(|| {
            Raise::at(
                line,
                format!(
                    "ValueError: could not convert string to float: {}",
                    pyfmt::repr(b)
                ),
            )
        }),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: float() argument must be a string or a real number, not '{}'",
                pyfmt::type_name(other)
            ),
        )),
    }
}

/// `gb(b)`: decimal units, one place below ten of the unit.
pub(super) fn gb(b: &Value, line: u32) -> Result<String, Raise> {
    if *b == Value::Null {
        return Ok("\u{2014}".to_string());
    }
    let f = float(b, line)?;
    for (unit, size) in [("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3)] {
        if f >= size {
            let places = if f < 10.0 * size { 1 } else { 0 };
            return Ok(format!("{} {unit}", py::f(f / size, 0, places)));
        }
    }
    Ok(format!(
        "{} B",
        crate::families::py_d(&Value::Float(f), line)?
    ))
}

/// `gib(b)`: GiB, whole when within 0.05 of a whole number.
fn gib(b: &Value, line: u32) -> Result<String, Raise> {
    if *b == Value::Null {
        return Ok("\u{2014}".to_string());
    }
    let g = float(b, line)? / f64::from(1u32 << 30);
    // `round(g)` raises for NaN and the infinities.
    pylib::ops::round_int(g, line)?;
    let places = if (g - g.round_ties_even()).abs() < 0.05 {
        0
    } else {
        1
    };
    Ok(format!("{} GiB", py::f(g, 0, places)))
}

/// `ago(t)`: how long ago `t` was, by the clock now.
fn ago(t: &Value, line: u32) -> Result<String, Raise> {
    if !py::truthy(t) {
        return Ok("never".to_string());
    }
    let x = match t {
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Int(digits) => digits.parse().unwrap_or(f64::INFINITY),
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: unsupported operand type(s) for -: 'float' and '{}'",
                    pyfmt::type_name(other)
                ),
            ))
        }
    };
    // `int(time.time() - t)`.
    let s = (py::now() - x).trunc();
    let whole = |v: f64| crate::families::py_d(&Value::Float(v), line);
    if s.is_nan() || s.is_infinite() {
        whole(s)?;
    }
    Ok(if s < 120.0 {
        format!("{} s ago", whole(s)?)
    } else if s < 7200.0 {
        format!("{} min ago", whole((s / 60.0).floor())?)
    } else {
        format!("{} h ago", whole((s / 3600.0).floor())?)
    })
}

/// `who(r)`.
fn who(r: &Value, line: u32) -> Result<String, Raise> {
    let o = dict_at(r, "owner", line)?;
    let handle = py::dget(&o, "handle", line)?;
    if py::truthy(&handle) {
        return Ok(py::s(&handle));
    }
    Ok(if py::dget(&o, "kind", line)? == py::text("server") {
        "server".to_string()
    } else {
        "\u{2014}".to_string()
    })
}

/// `-x` for `sorted`'s key.
fn neg(x: &Value, line: u32) -> Result<f64, Raise> {
    match x {
        Value::Float(f) => Ok(-f),
        Value::Bool(b) => Ok(-f64::from(u8::from(*b))),
        Value::Int(digits) => Ok(-digits.parse::<f64>().unwrap_or(f64::INFINITY)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: bad operand type for unary -: '{}'",
                pyfmt::type_name(other)
            ),
        )),
    }
}

/// `parts(r, n)`: the `n` biggest parts, biggest first.
fn parts(r: &Value, n: usize, line: u32) -> Result<String, Raise> {
    let p = dict_at(r, "parts", line)?;
    let Value::Object(pairs) = &p else {
        return Err(Raise::no_attribute(line, &p, "items"));
    };
    let mut keyed = Vec::with_capacity(pairs.len());
    for (k, x) in pairs {
        keyed.push((neg(x, line)?, k, x));
    }
    keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    for (_, k, x) in keyed.into_iter().take(n) {
        out.push(format!(
            "{} {}",
            py::s(&Value::Str(k.clone())),
            gb(x, line)?
        ));
    }
    Ok(out.join(" \u{b7} "))
}

/// `r.get("kind") in kinds`.
fn kind_in(r: &Value, kinds: &[&str], line: u32) -> Result<bool, Raise> {
    let k = py::dget(r, "kind", line)?;
    Ok(kinds.iter().any(|want| k == py::text(want)))
}

/// `seq[0]`.
fn first(v: &Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => Ok(items[0].clone()),
        Value::Str(_) => py::head(v, 1, line),
        Value::Object(_) => Err(Raise::at(line, "KeyError: 0")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                pyfmt::type_name(other)
            ),
        )),
    }
}

/// `_app_resources_print`'s Python.
pub fn lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let b = dict_at(&v, "box", 26)?;
    let g = dict_at(&v, "guard", 26)?;
    let scan = dict_at(&v, "scan", 26)?;
    let l = dict_at(&g, "lines", 27)?;
    let env = py::dget(&g, "modeSource", 28)?;
    let mode = plus(
        &py::dget_or(&g, "mode", py::text("?"), 28)?,
        if env == py::text("env") {
            " (from the environment)"
        } else {
            ""
        },
        28,
    )?;
    let host = py::or(py::dget(&b, "host", 30)?, py::text("this box"));
    let cores = py::or(py::dget(&b, "cores", 30)?, py::text("?"));
    let mem = gib(&py::dget(&b, "memBytes", 30)?, 30)?;
    // `time.localtime(b.get("checkedAt") or time.time())`: now when falsy.
    let at = py::dget(&b, "checkedAt", 31)?;
    let at = if py::truthy(&at) { at } else { Value::Null };
    let checked = py::local_time(&at, "%H:%M", 31)?;
    let scanned = ago(&py::dget(&scan, "workspacesAt", 31)?, 31)?;
    o.line(&format!(
        "{} \u{b7} {} cores \u{b7} {mem} RAM \u{b7} disk guard {mode} \u{b7} checked {checked} (folders scanned {scanned})",
        py::s(&host),
        py::s(&cores)
    ));
    let rows = py::or(py::dget(&v, "consumers", 32)?, py::empty_list());
    o.line("");
    o.line("DISK");
    let disks = py::or(py::dget(&v, "disks", 35)?, py::empty_list());
    for d in py::iter(&disks, 35)? {
        let at = py::or(py::dget(&d, "mount", 36)?, py::dget(&d, "id", 36)?);
        let used = gb(&py::dget(&d, "usedBytes", 36)?, 36)?;
        let total = gb(&py::dget(&d, "totalBytes", 36)?, 36)?;
        let pct = py::dget(&d, "usedPct", 37)?;
        let free = gb(&py::dget(&d, "freeBytes", 37)?, 37)?;
        let level = py::s(&py::dget_or(&d, "level", py::text("?"), 37)?).to_uppercase();
        o.line(&format!(
            "  {} {used} used of {total} ({}%) \u{b7} {free} free \u{b7} {level}",
            py::left(&at, 8),
            py::s(&pct)
        ));
        if py::truthy(&py::dget(&d, "holds", 38)?) {
            o.line(&format!(
                "           holds: {}",
                py::join(", ", &py::idx(&d, "holds", 39)?, 39)?
            ));
        }
    }
    let warn = py::dget(&l, "warnPct", 41)?;
    let warn_free = gib(&py::dget(&l, "warnFreeBytes", 41)?, 41)?;
    let refuse = py::dget(&l, "refusePct", 41)?;
    let reserve = gib(&py::dget(&l, "reserveBytes", 41)?, 41)?;
    let critical = py::dget(&l, "criticalPct", 42)?;
    let critical_free = gib(&py::dget(&l, "criticalFreeBytes", 42)?, 42)?;
    o.line(&format!(
        "           Lines: warn {}% or {warn_free} free \u{b7} refuse {}% or {reserve} left \u{b7} critical {}% or {critical_free} free",
        py::s(&warn),
        py::s(&refuse),
        py::s(&critical)
    ));
    let m = dict_at(&v, "mem", 43)?;
    let mut ram = vec![format!(
        "{} total",
        gib(&py::dget(&m, "totalBytes", 45)?, 45)?
    )];
    if py::dget(&m, "availableBytes", 46)? != Value::Null {
        ram.push(format!(
            "{} available",
            gib(&py::idx(&m, "availableBytes", 46)?, 46)?
        ));
    } else {
        ram.push("available not measured on this OS".to_string());
    }
    if py::dget(&m, "agentsBytes", 47)? != Value::Null {
        ram.push(format!(
            "agents {}",
            gib(&py::idx(&m, "agentsBytes", 48)?, 48)?
        ));
    }
    o.line(&format!("RAM        {}", ram.join(" \u{b7} ")));
    let cpu = dict_at(&v, "cpu", 50)?;
    let load = py::or(py::dget(&cpu, "load5", 50)?, py::text("\u{2014}"));
    let cores = py::or(py::dget(&b, "cores", 50)?, py::text("?"));
    o.line(&format!(
        "CPU        load {} (5 min) on {} cores",
        py::s(&load),
        py::s(&cores)
    ));
    let sections: [(&[&str], &str); 4] = [
        (&["app", "service"], "APPS AND SERVICES"),
        (&["database"], "DATABASES"),
        (&["workspace"], "AGENT FOLDERS"),
        (&["shared"], "SHARED"),
    ];
    let rows = py::iter(&rows, 59)?;
    for (kinds, title) in sections {
        let mut sel = Vec::new();
        for r in &rows {
            if kind_in(r, kinds, 59)? {
                sel.push(r.clone());
            }
        }
        if sel.is_empty() {
            continue;
        }
        o.line("");
        if kinds.contains(&"app") {
            o.line(&format!(
                "{} {} {} {} {} STATE",
                crate::families::ljust(title, 28),
                crate::families::ljust("KIND", 15),
                crate::families::ljust("OWNER", 14),
                crate::families::ljust("DISK", 9),
                crate::families::ljust("RAM", 9)
            ));
        } else {
            o.line(title);
        }
        for r in &sel {
            if kind_in(r, &["app", "service"], 68)? {
                let k = if py::dget(r, "kind", 69)? == py::text("app") {
                    format!(
                        "app \u{b7} {}",
                        py::s(&py::dget_or(r, "path", py::text("?"), 69)?)
                    )
                } else {
                    "service".to_string()
                };
                let mut state = py::or(py::dget(r, "state", 70)?, py::text(""));
                if py::dget(r, "kind", 71)? == py::text("service")
                    && py::dget(r, "walled", 71)? == Value::Bool(false)
                {
                    state = Value::Str(Text::from(plus(&state, " \u{b7} unwalled --cmd", 72)?));
                }
                let id = py::dget(r, "id", 73)?;
                let owner = who(r, 73)?;
                let disk = gb(&py::dget(r, "diskBytes", 73)?, 73)?;
                let mem = gib(&py::dget(r, "memBytes", 73)?, 73)?;
                o.line(&format!(
                    "{} {} {} {} {} {}",
                    py::left(&id, 28),
                    crate::families::ljust(&k, 15),
                    crate::families::ljust(&owner, 14),
                    crate::families::ljust(&disk, 9),
                    crate::families::ljust(&mem, 9),
                    py::s(&state)
                ));
                if py::truthy(&py::dget(r, "parts", 74)?) {
                    o.line(&format!("    {}", parts(r, 6, 75)?));
                }
            } else {
                let mut extra = parts(r, 3, 77)?;
                if py::dget(r, "unusedBytes", 78)? != Value::Null {
                    let unused = gb(&py::idx(r, "unusedBytes", 79)?, 79)?;
                    let head = if extra.is_empty() {
                        String::new()
                    } else {
                        format!("{extra} \u{b7} ")
                    };
                    extra = format!("{head}{unused} unused");
                }
                if py::truthy(&py::dget(r, "folder", 80)?) {
                    let head = if extra.is_empty() {
                        String::new()
                    } else {
                        format!("{extra}  ")
                    };
                    extra = py::concat(&head, &py::idx(r, "folder", 81)?, 81)?;
                }
                let id = py::dget(r, "id", 82)?;
                let owner = who(r, 82)?;
                let disk = gb(&py::dget(r, "diskBytes", 82)?, 82)?;
                o.line(&format!(
                    "{} {} {} {extra}",
                    py::left(&id, 44),
                    crate::families::ljust(&owner, 14),
                    crate::families::ljust(&disk, 9)
                ));
            }
            if py::truthy(&py::dget(r, "partial", 83)?) {
                o.line("    (partial: the scan ran out of time)");
            }
        }
    }
    for d in py::iter(&disks, 85)? {
        if py::dget(&d, "notMeasuredBytes", 86)? != Value::Null {
            let n = gb(&py::idx(&d, "notMeasuredBytes", 87)?, 87)?;
            let mount = py::dget(&d, "mount", 87)?;
            o.line(&format!(
                "{} {} ~{n}  ({})",
                crate::families::ljust(NOT_MEASURED, 44),
                crate::families::ljust("", 14),
                py::s(&mount)
            ));
        }
    }
    let by = py::or(py::dget(&v, "byOwner", 88)?, py::empty_list());
    if py::truthy(&by) {
        o.line("");
        let mut biggest = Vec::new();
        for x in py::iter(&py::head(&by, 3, 91)?, 91)? {
            let owner = dict_at(&x, "owner", 91)?;
            let handle = py::or(py::dget(&owner, "handle", 91)?, py::text("?"));
            let disk = gb(&py::dget(&x, "diskBytes", 91)?, 91)?;
            biggest.push(format!("{} {disk}", py::s(&handle)));
        }
        o.line(&format!("BIGGEST OWNERS   {}", biggest.join(" \u{b7} ")));
        let top = py::dget(&dict_at(&first(&by, 92)?, "owner", 92)?, "address", 92)?;
        if py::truthy(&top) {
            o.line(&format!("{ASK}{} \"\u{2026}\"", py::s(&top)));
        }
    }
    let w = dict_at(&v, "k2WillClear", 95)?;
    if py::truthy(&py::dget(&w, "seedsBytes", 96)?) {
        let bytes = gb(&py::idx(&w, "seedsBytes", 97)?, 97)?;
        let seeds = py::dget(&w, "seeds", 97)?;
        o.line(&format!(
            "{CLEAR}{bytes} of old Seeds ({}) at the next sweep",
            py::s(&seeds)
        ));
    }
    if !py::truthy(&py::dget(&scan, "workspacesAt", 98)?)
        && !py::truthy(&py::dget(&scan, "appsAt", 98)?)
    {
        o.line("");
        o.line("No scan has measured Apps and folders yet: their disk shows as \u{2014}.");
    }
    if py::truthy(&py::dget(&v, "suggestion", 101)?) {
        o.line("");
        o.line(&py::s(&py::idx(&v, "suggestion", 103)?));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_as_the_python_writes_them() {
        let n = |t: &str| crate::out::json::parse(t).unwrap();
        assert_eq!(gb(&Value::Null, 1).unwrap(), "\u{2014}");
        assert_eq!(gb(&n("999"), 1).unwrap(), "999 B");
        assert_eq!(gb(&n("1500"), 1).unwrap(), "1.5 KB");
        assert_eq!(gb(&n("15000000"), 1).unwrap(), "15 MB");
        assert_eq!(gb(&n("\"2e12\""), 1).unwrap(), "2.0 TB");
        assert_eq!(gb(&n("-5.5"), 1).unwrap(), "-5 B");
        assert!(gb(&n("[1]"), 1).is_err());
        assert_eq!(gib(&n("8589934592"), 1).unwrap(), "8 GiB");
        assert_eq!(gib(&n("1610612736"), 1).unwrap(), "1.5 GiB");
        assert_eq!(ago(&n("0"), 1).unwrap(), "never");
        assert_eq!(ago(&Value::Null, 1).unwrap(), "never");
    }

    #[test]
    fn parts_biggest_first_ties_in_order() {
        let r = crate::out::json::parse(r#"{"parts":{"a":1000,"b":3000,"c":3000,"d":5}}"#).unwrap();
        assert_eq!(
            parts(&r, 3, 1).unwrap(),
            "b 3.0 KB \u{b7} c 3.0 KB \u{b7} a 1.0 KB"
        );
    }
}
