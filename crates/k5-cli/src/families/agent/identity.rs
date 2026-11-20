//! `cmd_agent_identity` (`cli/src/talk-and-workspace.sh`): an agent's
//! name, color and picture through `/cli/agent/identity`
//! (`cli_send_json`), and `cmd_agent_access` (`cli/src/app-guests.sh`):
//! one workspace's agent switches through `/cli/agent-access/set`
//! (`_compute_call`).

use super::{cli, dumps, obj, product, s};
use crate::exit::Exit;
use crate::families::app::call::Ctx;
use crate::families::approve::cp;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::client::Error;
use crate::transport::encode::quote;
use crate::transport::Client;

pub const IDENTITY: &str = "/cli/agent/identity";
pub const ACCESS: &str = "/cli/agent-access/set";
/// The shell's tool id for `agent identity` (an `id` tool).
pub const IDENTITY_TOOL: &str = "agent-identity";

/// `agent identity`'s words, the target made absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityArgs {
    pub target: String,
    pub name: Option<String>,
    pub color: String,
    pub image: String,
    pub clear_image: bool,
    pub json: bool,
}

fn identity_usage() -> String {
    format!(
        "Usage: {} agent identity [<agent>] [--color <hex|rgb>] [--image <file.png|jpg|webp> | --clear-image] [--name <name>] [--json]",
        cli()
    )
}

/// What `cmd_agent_identity` stops at before it asks: its lines on
/// stderr, exit 2.
pub type Stop = Vec<String>;

/// `cmd_agent_identity`'s words and checks, up to the request. `pwd` is
/// `$PWD`. `None`: a target the binary can't make absolute as the shell
/// does (hand the line back).
pub fn parse_identity(rest: &[&str], pwd: &str) -> Option<Result<IdentityArgs, Stop>> {
    let usage = identity_usage();
    let mut a = IdentityArgs {
        target: String::new(),
        name: None,
        color: String::new(),
        image: String::new(),
        clear_image: false,
        json: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--name" | "--color" | "--image" => {
                let Some(v) = rest.get(i + 1) else {
                    return Some(Err(vec![usage]));
                };
                let v = (*v).to_string();
                match w {
                    "--name" => a.name = Some(v),
                    "--color" => a.color = v,
                    _ => a.image = v,
                }
                i += 1;
            }
            "--clear-image" => a.clear_image = true,
            "--json" => a.json = true,
            _ if w.starts_with('-') => return Some(Err(vec![format!("Unknown flag: {w}"), usage])),
            _ if a.target.is_empty() => a.target = w.to_string(),
            _ => return Some(Err(vec![usage])),
        }
        i += 1;
    }
    if !a.image.is_empty() && a.clear_image {
        return Some(Err(vec![
            "Use --image or --clear-image, not both.".to_string()
        ]));
    }
    if !a.image.is_empty() && !std::path::Path::new(&a.image).is_file() {
        return Some(Err(vec![format!("Error: no such file: {}", a.image)]));
    }
    if a.target.contains('/') {
        a.target = crate::families::workspace::register::abs_path(&a.target, pwd)?;
    }
    Some(Ok(a))
}

/// `rgb_to_hex(s)`: `rgb(r,g,b)`, `rgba(…)` or `r,g,b` as `#rrggbb`; else
/// `s` as it was.
pub fn rgb_to_hex(s: &str) -> String {
    let t = super::py_strip(s);
    let groups = rgb_call(t).or_else(|| bare_rgb(t));
    match groups {
        Some(g) if g.iter().all(|n| *n <= 255) => {
            format!("#{:02x}{:02x}{:02x}", g[0], g[1], g[2])
        }
        _ => s.to_string(),
    }
}

/// Python's `\s` (a str pattern): Unicode whitespace.
fn ws(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

/// One to three ASCII digits at the start of `t`, followed by no digit:
/// the number and what is left.
fn digits(t: &str) -> Option<(u32, &str)> {
    let n = t.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=3).contains(&n) {
        return None;
    }
    Some((t[..n].parse().ok()?, &t[n..]))
}

/// `\s*[, ]\s*` is the whole of `sep`.
fn comma_or_space(sep: &str) -> bool {
    let commas = sep.matches(',').count();
    let rest_ws = sep.chars().filter(|c| *c != ',').all(ws);
    rest_ws && (commas == 1 || (commas == 0 && sep.contains(' ')))
}

/// The text up to the next ASCII digit, and the rest.
fn to_digit(t: &str) -> (&str, &str) {
    let i = t.find(|c: char| c.is_ascii_digit()).unwrap_or(t.len());
    (&t[..i], &t[i..])
}

/// `rgba?\(\s*D\s*[, ]\s*D\s*[, ]\s*D(?:\s*[,/]\s*[0-9.]+\s*)?\)`, any case.
fn rgb_call(t: &str) -> Option<[u32; 3]> {
    let lower = t.to_ascii_lowercase();
    let skip = if lower.starts_with("rgba(") {
        5
    } else if lower.starts_with("rgb(") {
        4
    } else {
        return None;
    };
    let inner = t.get(skip..)?.strip_suffix(')')?;
    let inner = inner.trim_start_matches(ws);
    let (a, rest) = digits(inner)?;
    let (sep, rest) = to_digit(rest);
    if !comma_or_space(sep) {
        return None;
    }
    let (b, rest) = digits(rest)?;
    let (sep, rest) = to_digit(rest);
    if !comma_or_space(sep) {
        return None;
    }
    let (c, rest) = digits(rest)?;
    let tail = rest.trim_matches(ws);
    if !tail.is_empty() {
        let after = tail.strip_prefix([',', '/'])?.trim_start_matches(ws);
        if after.is_empty() || !after.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
            return None;
        }
    }
    Some([a, b, c])
}

/// `D\s*,\s*D\s*,\s*D`.
fn bare_rgb(t: &str) -> Option<[u32; 3]> {
    let (a, rest) = digits(t)?;
    let (sep, rest) = to_digit(rest);
    if !(sep.matches(',').count() == 1 && sep.chars().filter(|c| *c != ',').all(ws)) {
        return None;
    }
    let (b, rest) = digits(rest)?;
    let (sep, rest) = to_digit(rest);
    if !(sep.matches(',').count() == 1 && sep.chars().filter(|c| *c != ',').all(ws)) {
        return None;
    }
    let (c, rest) = digits(rest)?;
    rest.is_empty().then_some([a, b, c])
}

/// `cmd_agent_identity` past its words: the request and its reader.
pub fn identity(client: &mut Client, closing: &mut Closing, a: &IdentityArgs, project: &str, agent_passport: bool) -> Exit {
    let mut target = a.target.clone();
    if target.is_empty() && !agent_passport {
        target = project.to_string();
    }
    let post = a.name.is_some() || !a.color.is_empty() || !a.image.is_empty() || a.clear_image;
    let payload = if post {
        match identity_body(&target, a) {
            Ok(p) => p,
            Err(shown) => return shown.print(),
        }
    } else if !target.is_empty() {
        format!("workspace={}", quote(&target))
    } else {
        String::new()
    };
    let passport = client.passport().is_some();
    let (status, body) = match client.send_json(post, IDENTITY, &payload) {
        Ok(r) => {
            closing.answer(&r, passport);
            let mut raw = b"\n000".repeat(client.refused_before_answer() as usize);
            raw.extend(r.body.iter().copied().filter(|b| *b != 0));
            (format!("{:03}", r.status), raw)
        }
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            return Exit::Error;
        }
        Err(_) => ("000".to_string(), Vec::new()),
    };
    identity_reader(&String::from_utf8_lossy(&body), &status, a.json, post).print()
}

/// The POST's JSON body, or the block's own stop (exit 2, or a traceback).
fn identity_body(target: &str, a: &IdentityArgs) -> Result<String, Shown> {
    let mut body = Vec::new();
    if !target.is_empty() {
        body.push(("workspace", s(target)));
    }
    if let Some(name) = &a.name {
        body.push(("displayName", s(name)));
    }
    if !a.color.is_empty() {
        body.push(("color", s(&rgb_to_hex(&a.color))));
    }
    if a.clear_image {
        body.push(("image", Value::Null));
    } else if !a.image.is_empty() {
        let raw = std::fs::read(&a.image).map_err(|e| {
            let f = pyjson::os_error(&e, &a.image);
            Shown::err(
                Raise::at(12, format!("{}: {}", f.class, f.message)).text(),
                Exit::Error,
            )
        })?;
        if raw.len() > 2 * 1024 * 1024 {
            return Err(Shown::err(
                "Error: the picture must be at most 2 MiB\n",
                Exit::Usage,
            ));
        }
        let mime = if raw.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if raw.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if raw.get(..4) == Some(b"RIFF") && raw.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            return Err(Shown::err(
                "Error: the picture must be PNG, JPEG or WebP\n",
                Exit::Usage,
            ));
        };
        let b64 = crate::families::store::widget::b64encode(&raw);
        body.push(("image", s(&format!("data:{mime};base64,{b64}"))));
    }
    Ok(dumps(&obj(body)))
}

/// The identity reader on `body` and its status (`rpartition("\n")` of
/// the shell's `<body>\n<status>`).
pub fn identity_reader(body: &str, status: &str, as_json: bool, post: bool) -> Shown {
    if status == "000" || super::py_strip(body).is_empty() {
        return Shown::err(
            format!(
                "Error: no response from the {} daemon (is it running?)\n",
                product()
            ),
            Exit::Error,
        );
    }
    let Ok(d) = pyjson::loads(body) else {
        return Shown::err(
            format!(
                "Error: unexpected daemon response (HTTP {status}): {}\n",
                super::cut(body, 200)
            ),
            Exit::Error,
        );
    };
    match identity_lines(&d, status, as_json, post) {
        Ok(shown) => shown,
        Err(r) => Shown::err(r.text(), Exit::Error),
    }
}

fn identity_lines(d: &Value, status: &str, as_json: bool, post: bool) -> Result<Shown, Raise> {
    let l = 9;
    let err = match d {
        Value::Object(_) => super::at(d, "error").cloned(),
        _ => None,
    };
    let err_set = err.as_ref().is_some_and(|e| *e != Value::Null);
    if err_set || !status.starts_with('2') {
        let get = |k: &str| crate::families::py_get(d, k, l);
        let mut msg = match get("message")? {
            Some(m) if py::truthy(m) => py::str_of(m),
            _ => match &err {
                Some(e) if py::truthy(e) => py::str_of(e),
                _ => format!("HTTP {status}"),
            },
        };
        if err.as_ref().is_some_and(|e| ops::py_eq(e, &s("rate_limited"))) {
            msg = format!(
                "too many changes; try again in {}s",
                super::str_or(get("retryAfterSecs")?, "?")
            );
        }
        let exit = match status {
            "403" | "401" => Exit::Refused,
            "404" => Exit::NotFound,
            _ => Exit::Error,
        };
        return Ok(Shown::err(format!("Error: {msg}\n"), exit));
    }
    if as_json {
        return Ok(Shown::out(dumps(d) + "\n"));
    }
    let get = |k: &str| crate::families::py_get(d, k, l);
    let mut out = String::new();
    let handle = super::str_or(get("handle")?, "");
    if post {
        let changed = match get("changed")? {
            Some(c) if py::truthy(c) => format!("{} updated", ops::join(c, ", ", l)?),
            _ => "no change".to_string(),
        };
        out.push_str(&format!("Agent {handle}: {changed}\n"));
    } else {
        out.push_str(&format!("Agent {handle}\n"));
    }
    out.push_str(&format!("  name:    {}\n", super::str_or(get("displayName")?, "")));
    let color = match get("color")? {
        Some(c) if py::truthy(c) => py::str_of(c),
        _ => "-".to_string(),
    };
    out.push_str(&format!("  color:   {color}\n"));
    let picture = if get("image")?.is_some_and(py::truthy) {
        "set"
    } else {
        "none"
    };
    out.push_str(&format!("  picture: {picture}\n"));
    if !post {
        let edit = get("canEditIdentity")?.is_some_and(py::truthy);
        let handle_ok = get("canEditHandle")?.is_some_and(py::truthy);
        let mut can = Vec::new();
        if edit {
            can.push(if handle_ok {
                "name, color, picture"
            } else {
                "color, picture"
            });
        }
        if handle_ok {
            can.push("handle");
        }
        let can = if can.is_empty() {
            "nothing".to_string()
        } else {
            can.join(", ")
        };
        out.push_str(&format!("  you can change: {can}\n"));
    }
    Ok(Shown::out(out))
}

// ---------------------------------------------------------------------
// `agent access`

/// `agent access set <ws> <toggle> on|off [--json]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessArgs {
    pub ws: String,
    pub toggle: String,
    pub on: bool,
    pub json: bool,
}

/// What `cmd_agent_access` does with its words: the request, or lines and
/// an exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    Set(AccessArgs),
    /// `agent access` alone: the usage and the switches on stdout, exit 2.
    Page,
    /// Lines on stderr, exit 2.
    Usage(Vec<String>),
}

fn access_usage() -> String {
    format!(
        "usage: {} agent access set <workspace> chat_manage|chat_refresh|agents_hire|agents_manage on|off [--json]",
        cli()
    )
}

/// `cmd_agent_access`'s words (a line with `-h` or `--help` is the
/// shell's).
pub fn parse_access(rest: &[&str]) -> Access {
    let usage = access_usage();
    match rest.first().copied() {
        None | Some("") => return Access::Page,
        Some("set") => {}
        Some(_) => return Access::Usage(vec![usage]),
    }
    let (mut ws, mut toggle, mut value, mut json) =
        (String::new(), String::new(), String::new(), false);
    for w in &rest[1..] {
        if *w == "--json" {
            json = true;
        } else if w.starts_with('-') {
            return Access::Usage(vec![format!("Unknown flag: {w}"), usage]);
        } else if ws.is_empty() {
            ws = (*w).to_string();
        } else if toggle.is_empty() {
            toggle = (*w).to_string();
        } else if value.is_empty() {
            value = (*w).to_string();
        } else {
            return Access::Usage(vec![usage]);
        }
    }
    let known = matches!(
        toggle.as_str(),
        "chat_manage" | "chat_refresh" | "agents_hire" | "agents_manage"
    );
    if ws.is_empty() || !known || !matches!(value.as_str(), "on" | "off") {
        return Access::Usage(vec![usage]);
    }
    Access::Set(AccessArgs {
        ws,
        toggle,
        on: value == "on",
        json,
    })
}

/// `agent access` alone: what it prints on stdout.
pub fn access_page() -> String {
    let cli = cli();
    format!(
        "{}\n  chat_manage = \"Manage chat and voice\": the agent may run chat and voice here, create Spaces and bind them to its Apps. On also makes its own chat account (off keeps it).\n  chat_refresh = \"Let this agent refresh its pinned chat\".\n  agents_hire = \"Let this agent hire agents\": its main session may run {cli} agent hire (a new agent in its own workspace).\n  agents_manage = \"Let this agent manage its sidecars\": its main session may open and stop its sidecars (also {cli} sidecar access on|off).\n",
        access_usage()
    )
}

/// `cmd_agent_access set …`: `_compute_call` and its reader.
pub fn access(client: Client, a: &AccessArgs) -> Exit {
    let mut ctx = Box::new(Ctx::new(client, false));
    let body = dumps(&obj(vec![
        ("workspace", s(&a.ws)),
        ("toggle", s(&a.toggle)),
        ("value", Value::Bool(a.on)),
    ]));
    let exit = match cp::call(&mut ctx, true, ACCESS, &body) {
        Err(e) => e,
        Ok(answer) if a.json => cp::body_line(&answer),
        Ok(answer) => access_reader(&answer).print(),
    };
    cp::close(ctx, exit)
}

/// The access reader on the answer.
pub fn access_reader(body: &[u8]) -> Shown {
    let l = 2;
    let v = match crate::families::app::py::load(body, l) {
        Ok(v) => v,
        Err(r) => return Shown::err(r.text(), Exit::Error),
    };
    let has = |k: &str| match &v {
        Value::Object(pairs) => crate::families::get(pairs, k).is_some(),
        Value::Array(items) => items.iter().any(|x| ops::py_eq(x, &s(k))),
        Value::Str(t) => py::str_of(&Value::Str(t.clone())).contains(k),
        _ => false,
    };
    let line = |label: &str, key: &str| -> Result<String, Raise> {
        let on = crate::families::py_get(&v, key, l)?.is_some_and(py::truthy);
        let ws = super::str_or(crate::families::py_get(&v, "workspace", l)?, "None");
        Ok(format!(
            "{label} is {} for {ws}\n",
            if on { "on" } else { "off" }
        ))
    };
    let text = if has("agentsCanRefreshChat") {
        line("Can refresh its own pinned chat", "agentsCanRefreshChat")
    } else if has("agentsCanHire") {
        line("Let this agent hire agents", "agentsCanHire")
    } else {
        match crate::families::py_get(&v, "toggle", l) {
            Ok(t) if t.is_some_and(|t| ops::py_eq(t, &s("agents_manage"))) => {
                line("Let this agent manage its sidecars", "agentsCanManage")
            }
            Ok(_) => line("Manage chat and voice", "agentsCanManageChat"),
            Err(r) => Err(r),
        }
    };
    match text {
        Ok(t) => Shown::out(t),
        Err(r) => Shown::err(r.text(), Exit::Error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_as_the_block_reads_them() {
        assert_eq!(rgb_to_hex("rgb(255, 0, 16)"), "#ff0010");
        assert_eq!(rgb_to_hex(" RGBA(1 2 3 / 0.5) "), "#010203");
        assert_eq!(rgb_to_hex("1,2,3"), "#010203");
        assert_eq!(rgb_to_hex("1, 2 ,3"), "#010203");
        assert_eq!(rgb_to_hex("256,0,0"), "256,0,0");
        assert_eq!(rgb_to_hex("rgb(1,,2,3)"), "rgb(1,,2,3)");
        assert_eq!(rgb_to_hex("1 2 3"), "1 2 3");
        assert_eq!(rgb_to_hex("#abc"), "#abc");
    }

    #[test]
    fn the_access_words() {
        assert_eq!(parse_access(&[]), Access::Page);
        assert!(matches!(parse_access(&["get"]), Access::Usage(_)));
        assert!(matches!(
            parse_access(&["set", "ws", "chat_manage", "on"]),
            Access::Set(_)
        ));
        assert!(matches!(
            parse_access(&["set", "ws", "chat_manage", "maybe"]),
            Access::Usage(_)
        ));
    }

    #[test]
    fn the_identity_reader() {
        let r = identity_reader(r##"{"handle":"qa","displayName":"QA","color":"#fff","canEditIdentity":true}"##, "200", false, false);
        assert_eq!(r.stdout, "Agent qa\n  name:    QA\n  color:   #fff\n  picture: none\n  you can change: color, picture\n");
        let r = identity_reader(r#"{"error":"rate_limited","retryAfterSecs":30}"#, "429", false, true);
        assert_eq!(r.stderr, "Error: too many changes; try again in 30s\n");
        let r = identity_reader("", "000", false, false);
        assert_eq!(r.exit, Exit::Error);
    }
}
