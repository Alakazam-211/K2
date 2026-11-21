//! `app access <workspace> [--build on|off] [--manage on|off] [--users
//! on|off] [--core <core>=on|off[,…]]… [--json]`: an agent's App
//! switches and cores, as the shell CLI answers it (`cmd_app_access`,
//! `cli/src/app-auth.sh`).
//!
//! - No change asked: one `_compute_call GET /cli/app/list
//!   workspace=<ws>`; `--json` is its `access` as Python's `json.dumps`
//!   writes it, else `_app_access_print`.
//! - A change: one `_compute_call POST /cli/agent-access/set` per switch,
//!   in the shell's order (switch 1 first when turning on, switch 2 first
//!   when turning off; then `--users`; then every core in one call), then
//!   the read again (`_compute_send`): its lines when it is a 200, else
//!   what changed. `--users` is checked only after the switches went.
//!
//! The credential is the shell's: the session's passport in a session,
//! the owner token outside one ([`Ctx::call`]); who may change what is
//! the daemon's to say.

use super::call::{Cj, Ctx};
use super::grace_set::{help, walk};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_space, Raise, Shown};
use crate::out;
use crate::out::json::{self, Options, Text, Value};
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const ACCESS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app access <workspace> [--build on|off] [--manage on|off] [--users on|off] [--core db|files|thread|mail|sew=on|off[,\u{2026}]]\u{2026} [--json]"
    );
    pub const CORE: &str =
        "usage: --core <core>=on|off[,<core>=on|off\u{2026}] (cores: db, files, thread, mail, sew)";
    pub const USERS: &str = "usage: --users on|off";
}

/// `--open`'s two lines (exit 2).
const OPEN_RETIRED: &str = concat!(
    cli_name!(),
    ": --open is retired: each open-path move (a new --cmd service, --path open, extra ports, broad egress) is held\n",
    "    for the owner or an admin, who approves it in person: ",
    cli_name!(),
    " app approvals, then ",
    cli_name!(),
    " app approve <id> (outside ",
    product_name!(),
    ")."
);

/// The line before `--manage` alone goes.
const MANAGE_NOTE: &str = concat!(
    cli_name!(),
    ": note: --manage is now switch 2 (\"\u{2026}and manage them end to end\"); \"Build and run Apps\" is --build."
);

/// `"%s: changed (k2 app access %s shows it once Apps are on)"`, split.
const CHANGED_MID: &str = concat!(": changed (", cli_name!(), " app access ");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Show {
        ws: String,
        json: bool,
    },
    Set {
        ws: String,
        build: String,
        manage: String,
        users: String,
        /// The cores' JSON (`{"db": true}`), when `--core` named any.
        cores: String,
        json: bool,
    },
}

/// The words after `access`. `None`: a help form (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(access(words))
}

fn access(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(
        words,
        &[
            &["--build"],
            &["--users"],
            &["--manage"],
            &["--open"],
            &["--core", "--cores"],
        ],
        &["--json"],
    )?;
    let ws = w.last();
    if ws.is_empty() {
        return Err(Stop::usage(usage::ACCESS));
    }
    // `cores="${cores:+$cores,}${2:-}"` for each `--core`.
    let mut cores = String::new();
    for v in w.all("--core") {
        if !cores.is_empty() {
            cores.push(',');
        }
        cores.push_str(&v);
    }
    let cores = if cores.is_empty() {
        String::new()
    } else {
        cores_json(&cores).ok_or_else(|| Stop::usage(usage::CORE))?
    };
    let (build, manage, users, open) = (
        w.v("--build"),
        w.v("--manage"),
        w.v("--users"),
        w.v("--open"),
    );
    let json = w.has("--json");
    if build.is_empty()
        && manage.is_empty()
        && users.is_empty()
        && cores.is_empty()
        && open.is_empty()
    {
        return Ok(Ask::Show { ws, json });
    }
    if !open.is_empty() {
        return Err(Stop::usage(OPEN_RETIRED));
    }
    for (t, v) in [("build", &build), ("manage", &manage)] {
        if !v.is_empty() && !matches!(v.as_str(), "on" | "off") {
            return Err(Stop::usage(format!("usage: --{t} on|off")));
        }
    }
    Ok(Ask::Set {
        ws,
        build,
        manage,
        users,
        cores,
        json,
    })
}

/// The cores' Python: `{core: on}` in the order named (a core named again
/// keeps its place, with the last word), or `None` (its `sys.exit(1)`).
fn cores_json(cores: &str) -> Option<String> {
    let mut out: Vec<(Text, Value)> = Vec::new();
    for part in cores.split(',') {
        let part = part.trim_matches(py_space);
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=')?;
        if !matches!(v, "on" | "off") || k.is_empty() {
            return None;
        }
        let key = Text::from(k.trim_matches(py_space));
        let on = Value::Bool(v == "on");
        match out.iter_mut().find(|(x, _)| *x == key) {
            Some(slot) => slot.1 = on,
            None => out.push((key, on)),
        }
    }
    json::to_string(&Value::Object(out), &Options::default()).ok()
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Show { ws, json } => {
            let body = ctx.call(false, "/cli/app/list", &format!("workspace={}", quote(ws)))?;
            if *json {
                return Ok(access_json(&body));
            }
            Ok(printed(&body, access_lines))
        }
        Ask::Set {
            ws,
            build,
            manage,
            users,
            cores,
            json,
        } => {
            if !manage.is_empty() && build.is_empty() {
                out::stderr_line(MANAGE_NOTE);
            }
            let order: [(&str, &String); 2] = if build == "off" {
                [("apps_manage_full", manage), ("apps_build", build)]
            } else {
                [("apps_build", build), ("apps_manage_full", manage)]
            };
            let mut last = Vec::new();
            for (toggle, v) in order {
                if v.is_empty() {
                    continue;
                }
                last = set(ctx, ws, toggle, Some(v == "on"), "")?;
            }
            if !users.is_empty() {
                if !matches!(users.as_str(), "on" | "off") {
                    return Ok(Shown::err(format!("{}\n", usage::USERS), Exit::Usage));
                }
                last = set(ctx, ws, "app_users", Some(users == "on"), "")?;
            }
            if !cores.is_empty() {
                last = set(ctx, ws, "app_core", None, cores)?;
            }
            if *json {
                return Ok(raw(&last));
            }
            let s = ctx.send(
                false,
                "/cli/app/list",
                &format!("workspace={}", quote(ws)),
                false,
            )?;
            if s.ok() {
                Ok(printed(&s.body, access_lines))
            } else {
                Ok(printed(&last, changed_lines))
            }
        }
    }
}

/// One `_compute_call POST /cli/agent-access/set`.
fn set(
    ctx: &mut Ctx,
    ws: &str,
    toggle: &str,
    value: Option<bool>,
    cores: &str,
) -> Result<Vec<u8>, Exit> {
    let mut b = Cj::new().s("workspace", ws).s("toggle", toggle);
    if let Some(on) = value {
        b = b.b("value", if on { "1" } else { "0" });
    }
    let b = b.j("cores", cores).body();
    ctx.call(true, "/cli/agent-access/set", &b)
}

fn printed(body: &[u8], lines: fn(&[u8], &mut Out) -> Result<(), Raise>) -> Shown {
    let mut o = Out::default();
    let r = lines(body, &mut o).map(|()| Exit::Ok);
    o.done(r)
}

/// `print(json.dumps(json.load(sys.stdin).get("access")))`.
fn access_json(body: &[u8]) -> Shown {
    let r = (|| -> Result<String, Raise> {
        let v = py::load(body, 1)?;
        let a = py::dget(&v, "access", 1)?;
        Ok(json::to_string(&a, &Options::default()).unwrap_or_default())
    })();
    match r {
        Ok(text) => Shown::out(format!("{text}\n")),
        Err(e) => crate::families::raised(String::new(), String::new(), &e),
    }
}

fn on_off(v: &Value) -> &'static str {
    if py::truthy(v) {
        "on"
    } else {
        "off"
    }
}

/// `_app_access_print`.
fn access_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let top = py::load(body, 3)?;
    let v = py::or(py::dget(&top, "access", 3)?, Value::Object(Vec::new()));
    let ws = py::dget(&v, "workspace", 4)?;
    let build = on_off(&py::dget(&v, "buildAndRun", 4)?);
    let full = on_off(&py::dget(&v, "manageEndToEnd", 4)?);
    let users = on_off(&py::dget(&v, "manageUsers", 4)?);
    o.line(&format!(
        "{}: Build and run Apps {build}, manage them end to end {full}, manage their users {users}",
        py::s(&ws)
    ));
    let list = py::or(py::dget(&v, "coreList", 5)?, py::empty_list());
    for c in py::iter(&list, 5)? {
        let later = py::dget(&c, "later", 6)?;
        let ticked = if py::truthy(&py::dget(&c, "ticked", 7)?) {
            "x"
        } else {
            " "
        };
        let core = py::dget(&c, "core", 7)?;
        let label = py::dget_or(&c, "label", core.clone(), 7)?;
        let core = py::dget(&c, "core", 7)?;
        let tail = if py::truthy(&later) {
            let t = py::concat("  (", &later, 7)?;
            format!("{t})")
        } else {
            String::new()
        };
        o.line(&format!(
            "  [{ticked}] {} {}{tail}",
            py::left(&label, 16),
            py::s(&core)
        ));
    }
    Ok(())
}

/// What changed, when the read again is not a 200.
fn changed_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let ws = py::dget(&v, "workspace", 4)?;
    o.line(&format!(
        "{}{CHANGED_MID}{} shows it once Apps are on)",
        py::s(&ws),
        py::s(&py::dget(&v, "workspace", 4)?)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cores_read_as_the_python_reads_them() {
        assert_eq!(
            cores_json("db=on, files =off,,db=off").as_deref(),
            Some(r#"{"db": false, "files": false}"#)
        );
        assert_eq!(cores_json(",").as_deref(), Some("{}"));
        assert_eq!(cores_json("db"), None);
        assert_eq!(cores_json("db= on"), None);
        assert_eq!(cores_json(" =on"), None);
    }

    #[test]
    fn the_loop_stops_where_the_shell_stops() {
        assert_eq!(
            parse(&["w", "--open", "yes", "--build", "maybe"]),
            Some(Err(Stop::usage(OPEN_RETIRED)))
        );
        assert_eq!(
            parse(&["w", "--manage", "x"]),
            Some(Err(Stop::usage("usage: --manage on|off")))
        );
        assert_eq!(
            parse(&["w", "--core", ""]),
            Some(Ok(Ask::Show {
                ws: "w".into(),
                json: false
            }))
        );
        assert_eq!(
            parse(&["w", "--users", "maybe"]),
            Some(Ok(Ask::Set {
                ws: "w".into(),
                build: String::new(),
                manage: String::new(),
                users: "maybe".into(),
                cores: String::new(),
                json: false
            }))
        );
    }
}
