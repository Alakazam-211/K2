//! The owner's switches, as `cmd_compute` answers them
//! (`cli/src/compute-command.sh`), each one POST over TCP and one line:
//!
//! - `revoke <node> --workspace WS`: `/cli/compute/grants/revoke`.
//! - `preview on|off` and `placement v1|v2`: `/cli/settings/update`
//!   (`computePreview`, `computePlacement`).
//! - `access on|off`, `operate on|off` and `manage on|off` [`--workspace
//!   WS`]: `/cli/agent-access/set`, the workspace's `compute`,
//!   `compute_operate` or `compute_manage` toggle (this checkout's
//!   workspace outside a session, the passport's in one).

use super::super::writes::{body_of, dumps, load, object, stopped, str_value, Cp, Words};
use super::{str_plus, table_get};
use crate::exit::Exit;
use crate::families::{py_d, Raise, Shown};
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::ops::{contains, get_in, index, join};
use k2_client::cli_name;

pub const REVOKE: &str = "/cli/compute/grants/revoke";
pub const SETTINGS: &str = "/cli/settings/update";
pub const ACCESS: &str = "/cli/agent-access/set";

pub const REVOKE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute revoke <node> --workspace WS"
);
pub const ACCESS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute access on|off --workspace WS"
);
pub const OPERATE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute operate on|off --workspace WS"
);
pub const MANAGE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute manage on|off --workspace WS"
);

/// The placement names `placement`'s reader explains.
const PLACEMENTS: &[(&str, &str)] = &[
    (
        "v2",
        "load bands, a warm slot worth one band, bursts spread",
    ),
    ("v1", "the 0.46.5 order: warm slot first, then free CPU"),
];

/// Which toggle of `/cli/agent-access/set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// `access`: "Allow compute nodes".
    Compute,
    /// `operate`: "Allow this agent to operate compute nodes".
    Operate,
    /// `manage`: "Allow managing compute nodes".
    Manage,
}

impl Toggle {
    fn wire(self) -> &'static str {
        match self {
            Toggle::Compute => "compute",
            Toggle::Operate => "compute_operate",
            Toggle::Manage => "compute_manage",
        }
    }

    fn usage(self) -> &'static str {
        match self {
            Toggle::Compute => ACCESS_USAGE,
            Toggle::Operate => OPERATE_USAGE,
            Toggle::Manage => MANAGE_USAGE,
        }
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Revoke {
        node: String,
        workspace: String,
    },
    Preview {
        on: bool,
    },
    Placement {
        version: String,
    },
    Toggle {
        toggle: Toggle,
        on: bool,
        /// `--workspace`; empty for `_compute_ws ""`.
        workspace: String,
    },
}

/// The words after the verb (`verb` is `revoke`, `preview`, `placement`,
/// `access`, `operate` or `manage`).
pub fn parse(verb: &str, words: &[&str]) -> Words<Ask> {
    match verb {
        "revoke" => revoke(words),
        "preview" => Words::Ask(Ask::Preview {
            on: words.first() == Some(&"on"),
        }),
        "placement" => Words::Ask(Ask::Placement {
            version: words.first().copied().unwrap_or("").to_string(),
        }),
        "access" => toggle(Toggle::Compute, words),
        "operate" => toggle(Toggle::Operate, words),
        _ => toggle(Toggle::Manage, words),
    }
}

fn revoke(words: &[&str]) -> Words<Ask> {
    let (mut node, mut workspace) = (String::new(), String::new());
    let mut i = 0;
    while i < words.len() {
        if words[i] == "--workspace" {
            workspace = words.get(i + 1).copied().unwrap_or("").to_string();
            if i + 2 > words.len() {
                return Words::ShiftFailed;
            }
            i += 2;
        } else {
            node = words[i].to_string();
            i += 1;
        }
    }
    if node.is_empty() || workspace.is_empty() {
        return Words::Usage(REVOKE_USAGE.to_string());
    }
    Words::Ask(Ask::Revoke { node, workspace })
}

/// `local action="${1:-…}"; shift 2>/dev/null || true`, the flags, then
/// `on|off`.
fn toggle(toggle: Toggle, words: &[&str]) -> Words<Ask> {
    let action = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or(&[]);
    let mut workspace = String::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--workspace" => {
                workspace = rest.get(i + 1).copied().unwrap_or("").to_string();
                if i + 2 > rest.len() {
                    return Words::ShiftFailed;
                }
                i += 2;
            }
            "--all-machines" if toggle == Toggle::Compute => i += 1,
            _ => return Words::Usage(toggle.usage().to_string()),
        }
    }
    match action {
        "on" | "off" => Words::Ask(Ask::Toggle {
            toggle,
            on: action == "on",
            workspace,
        }),
        _ => Words::Usage(toggle.usage().to_string()),
    }
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let (path, body) = match a {
        Ask::Revoke { node, workspace } => {
            (REVOKE, body_of(&[("node", node), ("workspace", workspace)]))
        }
        Ask::Preview { on } => (SETTINGS, format!("{{\"computePreview\":{on}}}")),
        Ask::Placement { version } => (SETTINGS, format!("{{\"computePlacement\":\"{version}\"}}")),
        Ask::Toggle {
            toggle,
            on,
            workspace,
        } => {
            let ws = cp.ws(workspace);
            let mut pairs = vec![];
            if !ws.is_empty() {
                pairs.push(("workspace", str_value(&ws)));
            }
            pairs.push(("toggle", str_value(toggle.wire())));
            pairs.push(("value", Value::Bool(*on)));
            (ACCESS, dumps(&object(pairs)))
        }
    };
    let answer = match cp.call(true, path, &body, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let shown = match a {
        Ask::Revoke { .. } => revoke_reader(&answer),
        Ask::Preview { .. } => preview_reader(&answer),
        Ask::Placement { .. } => placement_reader(&answer),
        Ask::Toggle { toggle, .. } => match toggle {
            Toggle::Compute => access_reader(&answer),
            Toggle::Operate => operate_reader(&answer),
            Toggle::Manage => manage_reader(&answer),
        },
    };
    shown.print()
}

/// A reader's one line, or what stopped it.
fn one_line(r: Result<String, Raise>) -> Shown {
    match r {
        Ok(line) => Shown::out(format!("{line}\n")),
        Err(e) => stopped(String::new(), &e),
    }
}

/// `print(("revoked %s for %s. " % (v["node"], v["workspace"]) if
/// v["removed"] else "no grant to revoke. ") + v["note"])`.
pub fn revoke_reader(body: &[u8]) -> Shown {
    one_line((|| -> Result<String, Raise> {
        let v = load(body, 1)?;
        let head = if py::truthy(index(&v, "removed", 1)?) {
            format!(
                "revoked {} for {}. ",
                py::str_of(index(&v, "node", 1)?),
                py::str_of(index(&v, "workspace", 1)?)
            )
        } else {
            "no grant to revoke. ".to_string()
        };
        str_plus(&head, index(&v, "note", 1)?, 1)
    })())
}

/// `"Compute nodes (preview): %s on this server"`.
pub fn preview_reader(body: &[u8]) -> Shown {
    one_line((|| -> Result<String, Raise> {
        let v = load(body, 1)?;
        let on = get_in(&v, "computePreview", 1)?.is_some_and(py::truthy);
        Ok(format!(
            "Compute nodes (preview): {} on this server",
            if on { "on" } else { "off" }
        ))
    })())
}

/// `"Compute placement: %s on this server (%s)"`.
pub fn placement_reader(body: &[u8]) -> Shown {
    one_line((|| -> Result<String, Raise> {
        let v = load(body, 3)?;
        let v2 = str_value("v2");
        let p = match get_in(&v, "computePlacement", 3)? {
            Some(x) if py::truthy(x) => x.clone(),
            _ => v2,
        };
        let what = table_get(PLACEMENTS, &p, 4)?;
        Ok(format!(
            "Compute placement: {} on this server ({what})",
            py::str_of(&p)
        ))
    })())
}

/// Python's `a + b` for `v.get("granted", []) + v.get("kept", [])`.
fn plus(a: &Value, b: &Value, line: u32) -> Result<Value, Raise> {
    let ty = py::type_name;
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            Ok(Value::Array(x.iter().chain(y).cloned().collect()))
        }
        (Value::Str(_), Value::Str(_)) => {
            Ok(str_value(&format!("{}{}", py::str_of(a), py::str_of(b))))
        }
        (Value::Array(_), _) => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate list (not \"{}\") to list",
                ty(b)
            ),
        )),
        (Value::Str(_), _) => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                ty(b)
            ),
        )),
        (
            Value::Int(_) | Value::Bool(_) | Value::Float(_),
            Value::Int(_) | Value::Bool(_) | Value::Float(_),
        ) => Ok(number_sum(a, b)),
        _ => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and '{}'",
                ty(a),
                ty(b)
            ),
        )),
    }
}

/// Two numbers added, only as far as the reader needs (whether the sum
/// is true; `join` then raises on it).
fn number_sum(a: &Value, b: &Value) -> Value {
    let f = |v: &Value| match v {
        Value::Bool(t) => f64::from(u8::from(*t)),
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::NAN),
        Value::Float(x) => *x,
        _ => 0.0,
    };
    Value::Float(f(a) + f(b))
}

/// `access`'s reader (`"Allow compute nodes: %s for %s%s"`).
pub fn access_reader(body: &[u8]) -> Shown {
    one_line((|| -> Result<String, Raise> {
        let v = load(body, 2)?;
        let on = py::truthy(index(&v, "agentsCanUseCompute", 3)?);
        let empty = Value::Array(vec![]);
        let granted = get_in(&v, "granted", 4)?.unwrap_or(&empty).clone();
        let kept = get_in(&v, "kept", 4)?.unwrap_or(&empty).clone();
        let m = plus(&granted, &kept, 4)?;
        let tail = if on {
            if py::truthy(&m) {
                format!(": {}", join(&m, ", ", 5)?)
            } else {
                ": no machines switched on yet (ones added later are granted)".to_string()
            }
        } else if contains(&v, "grantsRemoved", 5)? {
            let zero = Value::Int("0".into());
            let n = get_in(&v, "grantsRemoved", 5)?.unwrap_or(&zero);
            format!(" ({} grants removed)", py_d(n, 5)?)
        } else {
            String::new()
        };
        Ok(format!(
            "Allow compute nodes: {} for {}{tail}",
            if on { "on" } else { "off" },
            py::str_of(index(&v, "workspace", 6)?)
        ))
    })())
}

/// `operate`'s reader.
pub fn operate_reader(body: &[u8]) -> Shown {
    one_line((|| -> Result<String, Raise> {
        let v = load(body, 2)?;
        let on = py::truthy(index(&v, "agentsCanOperateCompute", 3)?);
        let ws = py::str_of(index(&v, "workspace", 3)?);
        let on2 = py::truthy(index(&v, "agentsCanOperateCompute", 3)?);
        Ok(format!(
            "Allow this agent to operate compute nodes: {} for {ws}{}",
            if on { "on" } else { "off" },
            if on2 {
                " (narrow, pause, update, restart; root actions still wait for you)"
            } else {
                ""
            }
        ))
    })())
}

/// `manage`'s reader.
pub fn manage_reader(body: &[u8]) -> Shown {
    one_line((|| -> Result<String, Raise> {
        let v = load(body, 2)?;
        let on = py::truthy(index(&v, "agentsCanManageCompute", 3)?);
        Ok(format!(
            "Allow managing compute nodes: {} for {}{}",
            if on { "on" } else { "off" },
            py::str_of(index(&v, "workspace", 4)?),
            if on {
                " (node add/confirm/rename/pause/retire/route, grant/revoke; each act is in Access Audit; preview and node remove stay yours)"
            } else {
                ""
            }
        ))
    })())
}
