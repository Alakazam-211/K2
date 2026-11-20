//! `cmd_agent_list` (`cli/src/agent.sh`): every agent on this server.

use super::conf::{request, unreachable_line};
use super::{at, cli, cut, dumps, err_json, obj, raised, s, str_or};
use crate::families::{ljust, py_get, py_iter, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::json as pyjson;
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::Client;

/// The block's lines (`<stdin>`).
mod line {
    pub const ERROR: u32 = 9;
    pub const ROWS: u32 = 31;
}

/// `cmd_agent_list`'s words: `--json`, or the usage line.
pub fn parse(rest: &[&str]) -> Result<bool, String> {
    let mut json = false;
    for w in rest {
        if *w == "--json" {
            json = true;
        } else {
            return Err(format!(
                "unexpected argument '{w}' \u{2014} usage: {} agent list [--json]",
                cli()
            ));
        }
    }
    Ok(json)
}

/// `cmd_agent_list`.
pub fn list(client: &mut Client, closing: &mut Closing, as_json: bool) -> Exit {
    let held = match request(client, closing, super::LIST, &[]) {
        Ok(h) => h,
        Err(exit) => return exit,
    };
    if held.raw.is_empty() {
        if held.denied {
            out::stderr_line(&err_json(
                "local_transport_denied",
                "local restriction, daemon health unobserved",
            ));
        } else {
            out::stderr_line(&unreachable_line());
        }
        return Exit::Error;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    reader(&String::from_utf8_lossy(&held.raw), as_json, &home).print()
}

/// The block on the answer `raw`; `home` is `HOME`.
pub fn reader(raw: &str, as_json: bool, home: &str) -> Shown {
    let Ok(d) = pyjson::loads(raw) else {
        return Shown::err(
            err_json(
                "bad_response",
                &format!(
                    "daemon returned an unparseable response: {}",
                    py::repr(&s(&cut(raw, 200)))
                ),
            ) + "\n",
            Exit::Error,
        );
    };
    let err = match py_get(&d, "error", line::ERROR) {
        Ok(e) => e.cloned().unwrap_or(Value::Null),
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    if py::truthy(&err) || !at(&d, "ok").is_some_and(py::truthy) {
        let err = match err {
            e @ Value::Object(_) => e,
            other => {
                let hint = if py::truthy(&other) {
                    py::str_of(&other)
                } else {
                    cut(raw, 200)
                };
                obj(vec![("code", s("error")), ("hint", s(&hint))])
            }
        };
        return Shown::err(dumps(&obj(vec![("error", err)])) + "\n", Exit::Error);
    }
    if as_json {
        return Shown::out(dumps(&d) + "\n");
    }
    match rows(&d, home) {
        Ok(Some(text)) => Shown::out(text),
        Ok(None) => Shown::out(format!(
            "No agents registered on this server. Hire one: {} agent hire <dir>\n",
            cli()
        )),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `os.path.expanduser("~")` from `HOME`: its trailing `/` off, `/` when
/// nothing is left.
fn user_home(home: &str) -> String {
    let h = home.trim_end_matches('/');
    if h.is_empty() {
        "/".to_string()
    } else {
        h.to_string()
    }
}

/// The printer's `tilde(p)`.
fn tilde(p: &str, home: &str) -> String {
    if p == home || p.starts_with(&format!("{home}/")) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

/// The table, or `None` for no agents.
fn rows(d: &Value, home: &str) -> Result<Option<String>, Raise> {
    let agents = match at(d, "agents") {
        Some(a) if py::truthy(a) => a.clone(),
        _ => return Ok(None),
    };
    let home = user_home(home);
    let mut rows: Vec<[String; 5]> = vec![["NAME", "MODE", "ENABLED", "LIVE", "PATH"].map(String::from)];
    for a in py_iter(&agents, line::ROWS)? {
        let name = str_or(py_get(&a, "name", line::ROWS)?, "?");
        let mode = str_or(py_get(&a, "mode", line::ROWS)?, "?");
        let yes = |v: Option<&Value>| if v.is_some_and(py::truthy) { "yes" } else { "no" };
        let enabled = yes(py_get(&a, "enabled", line::ROWS)?).to_string();
        let live = yes(py_get(&a, "live", line::ROWS)?).to_string();
        let path = tilde(&str_or(py_get(&a, "path", line::ROWS)?, "?"), &home);
        rows.push([name, mode, enabled, live, path]);
    }
    let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let widths = [width(0), width(1), width(2), width(3)];
    let mut text = String::new();
    for r in &rows {
        text.push_str(&format!(
            "{}  {}  {}  {}  {}\n",
            ljust(&r[0], widths[0]),
            ljust(&r[1], widths[1]),
            ljust(&r[2], widths[2]),
            ljust(&r[3], widths[3]),
            r[4]
        ));
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table() {
        let raw = r#"{"ok":true,"agents":[{"name":"qa","mode":"agent","enabled":true,"live":false,"path":"/home/u/qa"},{"name":"ops-long","path":"/srv/ops"}]}"#;
        assert_eq!(
            reader(raw, false, "/home/u/").stdout,
            "NAME      MODE   ENABLED  LIVE  PATH\nqa        agent  yes      no    ~/qa\nops-long  ?      no       no    /srv/ops\n"
        );
    }

    #[test]
    fn errors_and_none() {
        let r = reader(r#"{"ok":false}"#, false, "/h");
        assert_eq!(
            r.stderr,
            "{\"error\": {\"code\": \"error\", \"hint\": \"{\\\"ok\\\":false}\"}}\n"
        );
        let r = reader(r#"{"ok":true,"agents":[]}"#, false, "/h");
        assert!(r.stdout.starts_with("No agents registered"));
    }
}
