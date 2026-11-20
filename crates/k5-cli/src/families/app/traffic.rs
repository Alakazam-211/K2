//! What called an App's gateway, as the shell CLI's `cmd_app_audit`
//! (`cli/src/app-door.sh`) answers it: `app audit <app> [--since 1h]
//! [--verb V] [--denied] [--limit N] [--oldest-first] [--verify]`. The
//! shell's word loop, one `_compute_call`, `--json` as it came, else the
//! shell's Python line for line; with `--verify`, a broken chain is exit 1
//! after the lines.

use super::call::Ctx;
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_len, Raise};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage line (exit 2).
pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " app audit <app-id> [--since 1h] [--verb V] [--denied] [--limit N] [--oldest-first] [--verify] [--json]"
);

/// The verbs here.
pub const VERBS: &[&str] = &["audit"];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub query: String,
    pub verify: bool,
    pub json: bool,
}

/// The words after `audit`. `None`: a help form (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    Some(audit(words))
}

fn audit(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut since, mut verb, mut limit) =
        (String::new(), String::new(), String::new(), String::new());
    let (mut denied, mut verify, mut json, mut oldest) = (false, false, false, false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--since" => Some(&mut since),
            "--verb" => Some(&mut verb),
            "--limit" => Some(&mut limit),
            _ => None,
        };
        if let Some(slot) = slot {
            // `x="${2:-}"; shift 2`: with one word left, set -e ends it.
            *slot = rest
                .get(1)
                .ok_or_else(|| Stop::silent(Exit::Error))?
                .to_string();
            rest = &rest[2..];
            continue;
        }
        match w {
            "--oldest-first" => oldest = true,
            "--newest-first" => oldest = false,
            "--denied" => denied = true,
            "--verify" => verify = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(Stop::usage(format!("Unknown flag: {w}"))),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(USAGE));
    }
    let mut q = format!("app={}", quote(&app));
    if oldest {
        q.push_str("&oldest=1");
    }
    for (key, v) in [("since", &since), ("verb", &verb), ("limit", &limit)] {
        if !v.is_empty() {
            q.push_str(&format!("&{key}={}", quote(v)));
        }
    }
    if denied {
        q.push_str("&denied=1");
    }
    if verify {
        q.push_str("&verify=1");
    }
    Ok(Ask {
        query: q,
        verify,
        json,
    })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let body = ctx.call(false, "/cli/app/audit", &ask.query)?;
    let mut shown = super::access::answer(&body, ask.json, audit_lines);
    if shown.exit != Exit::Ok || !ask.verify {
        return Ok(shown);
    }
    // `sys.exit(0 if all(d.get("ok") for d in … .get("chain") or []) else 1)`.
    let r = (|| -> Result<Exit, Raise> {
        let v = py::load(&body, 1)?;
        let chain = py::or(py::dget(&v, "chain", 1)?, py::empty_list());
        for d in py::iter(&chain, 1)? {
            let ok = py::dget(&d, "ok", 1).map_err(|e| Raise {
                frames: vec![(1, "<module>"), (1, "<genexpr>")],
                exception: e.exception,
            })?;
            if !py::truthy(&ok) {
                return Ok(Exit::Error);
            }
        }
        Ok(Exit::Ok)
    })();
    let tail = Out::default().done(r);
    shown.stderr.push_str(&tail.stderr);
    shown.exit = tail.exit;
    Ok(shown)
}

/// `cmd_app_audit`'s Python.
pub fn audit_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let t = py::or(py::dget(&v, "totals", 4)?, Value::Object(Vec::new()));
    let app = py::idx(&v, "app", 5)?;
    let field = |k: &str| py::dget(&t, k, 5);
    let (calls_n, denied, bytes_in, bytes_out) = (
        field("calls")?,
        field("denied")?,
        field("bytesIn")?,
        field("bytesOut")?,
    );
    o.line(&format!(
        "{}: {} calls, {} refused, {} B in, {} B out",
        py::s(&app),
        py::s(&calls_n),
        py::s(&denied),
        py::s(&bytes_in),
        py::s(&bytes_out)
    ));
    let calls = py::or(py::dget(&v, "calls", 6)?, py::empty_list());
    if py::truthy(&calls) {
        let order = py::or(py::dget(&v, "order", 8)?, py::text("newest first"));
        let flip = if order == py::text("oldest first") {
            "--newest-first"
        } else {
            "--oldest-first"
        };
        let n = py_len(&calls, 10)?;
        o.line(&format!(
            "{n} call{}, {} ({flip} to flip):",
            if n == 1 { "" } else { "s" },
            py::s(&order)
        ));
    }
    for c in py::iter(&calls, 11)? {
        let at = py::div(&py::idx(&c, "atMs", 12)?, 1000.0, 12)?;
        let at = py::local_time(&Value::Float(at), "%Y-%m-%d %H:%M:%S", 12)?;
        let code = if py::truthy(&py::dget(&c, "code", 13)?) {
            py::concat(" ", &py::idx(&c, "code", 13)?, 13)?
        } else {
            String::new()
        };
        let get = |k: &str| py::dget(&c, k, 14);
        let (result, actor, verb, bin, bout, ms) = (
            get("result")?,
            get("actor")?,
            get("verb")?,
            get("bytesIn")?,
            get("bytesOut")?,
            get("latencyMs")?,
        );
        o.line(&format!(
            "{at}  {}{code}  {}  {}  in {} out {}  {} ms",
            py::right(&result, 3),
            py::s(&actor),
            py::s(&verb),
            py::s(&bin),
            py::s(&bout),
            py::s(&ms)
        ));
    }
    let chain = py::or(py::dget(&v, "chain", 15)?, py::empty_list());
    for d in py::iter(&chain, 15)? {
        let day = py::idx(&d, "day", 16)?;
        let state = if py::truthy(&py::dget(&d, "ok", 16)?) {
            format!("ok, {} rows", py::s(&py::idx(&d, "rows", 16)?))
        } else {
            format!("BROKEN at row {}", py::s(&py::dget(&d, "brokenAt", 16)?))
        };
        o.line(&format!("chain {}: {state}", py::s(&day)));
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        let a = parse(&[
            "acme",
            "--since",
            "1h",
            "--denied",
            "--oldest-first",
            "--limit",
            "5",
            "--verify",
            "--verb",
            "db.query",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            a.query,
            "app=acme&oldest=1&since=1h&verb=db.query&limit=5&denied=1&verify=1"
        );
        assert!(a.verify && !a.json);
        assert_eq!(parse(&[]), Some(Err(Stop::usage(USAGE))));
        assert_eq!(
            parse(&["a", "--since"]),
            Some(Err(Stop::silent(Exit::Error)))
        );
        assert_eq!(parse(&["a", "-h"]), None);
    }

    #[test]
    fn the_lines_are_the_shells() {
        let mut o = Out::default();
        let r = audit_lines(
            br#"{"app":"acme","totals":{"calls":2,"denied":1,"bytesIn":10,"bytesOut":20},"calls":[{"atMs":0,"result":403,"code":"scope","actor":"ann","verb":"db.query","bytesIn":1,"bytesOut":2,"latencyMs":3}],"order":"oldest first","chain":[{"day":"2026-11-20","ok":true,"rows":4},{"day":"2026-11-21","brokenAt":7}]}"#,
            &mut o,
        );
        assert_eq!(r.unwrap(), Exit::Ok);
        let lines: Vec<&str> = o.stdout.lines().collect();
        assert_eq!(lines[0], "acme: 2 calls, 1 refused, 10 B in, 20 B out");
        assert_eq!(lines[1], "1 call, oldest first (--newest-first to flip):");
        assert!(lines[2].ends_with("  403 scope  ann  db.query  in 1 out 2  3 ms"));
        assert_eq!(lines[3], "chain 2026-11-20: ok, 4 rows");
        assert_eq!(lines[4], "chain 2026-11-21: BROKEN at row 7");
    }
}
