//! `k5 workspace launch [<path|name>] [--json]`, `k5 workspace profile
//! [<path>]` and `k5 workspace update --field <f> --value <v>`, as
//! `cmd_workspace_launch`, `cmd_workspace_profile` and
//! `cmd_workspace_update` (`cli/src/agent-context.sh`) do them.
//!
//! - `launch`: the words first ([`parse_launch`]): `--json` is skipped
//!   (the answer is K2's JSON object either way), `--no-attach` and any
//!   other flag are refused, a second target too, each exit 2. The target
//!   is the word, or the workspace when there is none. When it is not a
//!   folder, `/cli/workspace/resolve q=<it>` asks the daemon for its path
//!   ([`resolved`]); an answer with no folder is `cli_print_or_fail`'s
//!   error when it has one, else the no-folder line, exit 1. Then a POST
//!   of `/cli/workspace/ensure-canonical-session` with the target as the
//!   one `project=`.
//! - `profile`: `/cli/workspace/agent-display-name` with the target (the
//!   first word, or the workspace) as `project=` and `project_path=` (the
//!   cell socket sends no `project=` of its own).
//! - `update`: the words ([`parse_update`]); a flag with no value, any
//!   other word, no field or value, and a projects DB column as the field
//!   are each lines and exit 2. Then the agent's name from
//!   `/cli/workspace/agent-display-name` ([`field`]; a refusal there is no
//!   closing line), or the workspace folder's name; then a POST of
//!   `/cli/agent/update agent=<name>&field=<f>&value=<v>`.
//!
//! Each request is `_ws_request` ([`super::ws_request`]), in its own
//! `$(…)`: a client each, so a passport one heals is not the one the next
//! starts with, as in bash. Each answer the command reports goes through
//! `cli_print_or_fail` ([`super::print_or_fail`]): an error on stderr with
//! its exit (3 with a permission card) and closing line, else the answer
//! as `$(…)` held it and a newline. A POST is sent once, as for
//! [`super::register`].
//!
//! The shell answers `update` with a workspace whose path is not absolute
//! or holds a newline (`basename` there is not sure).

use super::super::setup::{cut_200, utf8_locale};
use super::super::{get, py_space, s};
use super::{print_or_fail, verdict, ws_request, Stop, Verdict, Ws};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::py;
use crate::transport::encode::quote;
use crate::transport::{Call, Client};
use k2_client::brand::PRODUCT_NAME;
use k2_client::cli_name;

/// `launch`'s lookup.
pub const RESOLVE: &str = "/cli/workspace/resolve";
/// `launch`'s call.
pub const ENSURE: &str = "/cli/workspace/ensure-canonical-session";
/// `profile`'s call, and `update`'s lookup.
pub const DISPLAY_NAME: &str = "/cli/workspace/agent-display-name";
/// `update`'s call.
pub const AGENT_UPDATE: &str = "/cli/agent/update";
/// `update`'s usage line.
pub const UPDATE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace update --field <f> --value <v>"
);
/// `launch`'s usage line.
pub const LAUNCH_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace launch [<path|name>] [--json]"
);
/// `launch --no-attach`: exit 2.
pub const NO_ATTACH: &str = concat!(
    "Error: --no-attach: ", cli_name!(), " workspace launch never attaches a terminal (it starts or finds the workspace's agent session and prints ",
    k2_client::product_name!(),
    "'s answer), so there is nothing to turn off. Run it without --no-attach."
);

/// The projects DB columns `update` refuses as a field.
pub const DB_COLUMNS: [&str; 25] = [
    "id",
    "name",
    "path",
    "color",
    "tab_order",
    "last_opened_at",
    "worktree_mode",
    "icon_url",
    "focus_group_id",
    "pinned",
    "manually_active",
    "last_interaction_at",
    "created_at",
    "agent_enabled",
    "heartbeat_enabled",
    "agent_mode",
    "state_id",
    "tier_id",
    "heartbeat_mode",
    "heartbeat_schedule",
    "heartbeat_last_fire",
    "allow_remote_instruct",
    "use_session_stream",
    "sandbox_fs_mode",
    "anthropic_api_key",
];

/// A DB column as the field: exit 2.
pub fn db_column_lines(field: &str) -> Vec<String> {
    vec![
        format!("Error: '{field}' is a workspace SETTING (a projects DB column), not an AGENT.md profile field."),
        concat!("'", cli_name!(), " workspace update' only edits the agent's AGENT.md; it would not change the real setting.").to_string(),
        concat!("Use: ", cli_name!(), " agent set <name> [--mode <m>] [--enabled <bool>] [--name <new>]  (see '", cli_name!(), " agent set --help')").to_string(),
    ]
}

fn usage_stop(lines: Vec<String>) -> Stop {
    Stop {
        lines,
        exit: Exit::Usage,
    }
}

/// The words after `launch`, as its loop reads them: the target (empty
/// for none).
pub fn parse_launch(words: &[&str]) -> Result<String, Stop> {
    let mut target = String::new();
    for w in words {
        match *w {
            "--json" => {}
            "--no-attach" => return Err(usage_stop(vec![NO_ATTACH.to_string()])),
            w if w.starts_with('-') => {
                return Err(usage_stop(vec![
                    format!("Unknown flag: {w}"),
                    LAUNCH_USAGE.to_string(),
                ]))
            }
            w if !target.is_empty() => {
                return Err(usage_stop(vec![
                    format!("Unexpected extra argument: {w}"),
                    LAUNCH_USAGE.to_string(),
                ]))
            }
            w => target = w.to_string(),
        }
    }
    Ok(target)
}

/// `update`'s field and value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub field: String,
    pub value: String,
}

/// The words after `update`, as its loop reads them.
pub fn parse_update(words: &[&str]) -> Result<Update, Stop> {
    let usage = || UPDATE_USAGE.to_string();
    let (mut field, mut value) = (String::new(), String::new());
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--field" | "--value" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(usage_stop(vec![
                        format!("Error: {w} needs a value"),
                        usage(),
                    ]));
                };
                if *w == "--field" {
                    field = (*v).to_string();
                } else {
                    value = (*v).to_string();
                }
                i += 2;
            }
            w => {
                return Err(usage_stop(vec![
                    format!("Error: unknown argument: {w}"),
                    usage(),
                ]))
            }
        }
    }
    if field.is_empty() || value.is_empty() {
        return Err(usage_stop(vec![usage()]));
    }
    if DB_COLUMNS.contains(&field.as_str()) {
        return Err(usage_stop(db_column_lines(&field)));
    }
    Ok(Update { field, value })
}

/// `$(basename "$project")` for an absolute path with no newline in it.
pub fn basename(project: &str) -> Option<String> {
    if !project.starts_with('/') || project.contains('\n') {
        return None;
    }
    let trimmed = project.trim_end_matches('/');
    if trimmed.is_empty() {
        return Some("/".to_string());
    }
    Some(trimmed.rsplit('/').next().unwrap_or(trimmed).to_string())
}

/// `update`'s lookup Python on what `$(…)` held:
/// `print(json.load(sys.stdin).get(key, ""))` under a bare `except`, as
/// `$(…)` holds it (NULs and trailing newlines gone).
pub fn field(body: &[u8], key: &str) -> String {
    let text = String::from_utf8_lossy(body);
    let Ok(Value::Object(pairs)) = json::parse(&text) else {
        return String::new();
    };
    let v = get(&pairs, key).cloned().unwrap_or_else(|| s(""));
    // A string Python can't write out fails the print: nothing.
    if let Value::Str(t) = &v {
        if t.as_str().is_none() {
            return String::new();
        }
    }
    let printed: String = py::str_of(&v).chars().filter(|c| *c != '\0').collect();
    printed.trim_end_matches('\n').to_string()
}

/// `launch`'s resolve Python on what `$(…)` held: the answer's `path` when
/// it is a string with more than spaces in it, as `$(…)` holds what it
/// prints; else nothing.
pub fn resolved(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let Ok(Value::Object(pairs)) = json::parse(&text) else {
        return String::new();
    };
    let Some(Value::Str(p)) = get(&pairs, "path") else {
        return String::new();
    };
    let Some(p) = p.as_str() else {
        return String::new();
    };
    if p.chars().all(py_space) {
        return String::new();
    }
    let printed: String = p.chars().filter(|c| *c != '\0').collect();
    printed.trim_end_matches('\n').to_string()
}

/// `[[ -d "$target" ]]`.
fn is_dir(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// `${1:-$PROJECT}`, or `launch`'s target.
fn target(word: Option<String>, project: &str) -> String {
    word.filter(|w| !w.is_empty())
        .unwrap_or_else(|| project.to_string())
}

/// The line for a resolve answer with no folder, as bytes:
/// `${_WS_ANSWER:0:200}` may cut a character.
pub fn no_folder_line(target: &str, answer: &[u8], utf8: impl FnOnce() -> bool) -> Vec<u8> {
    let mut line = format!(
        "Error: {PRODUCT_NAME}'s server gave no folder for workspace '{target}' ({RESOLVE} answered: "
    )
    .into_bytes();
    line.extend_from_slice(cut_200(answer, utf8));
    line.extend_from_slice(b")\n");
    line
}

/// `launch`, after the checks: `lookup` for the resolve, `client` for the
/// POST.
pub fn launch(
    ws: &mut Ws,
    lookup: &mut Client,
    client: &mut Client,
    project: &str,
    word: String,
) -> Exit {
    let mut target = target(Some(word), project);
    if !is_dir(&target) {
        let q = format!("q={}", quote(&target));
        let answer = match ws_request(ws, lookup, RESOLVE, Call::Request(&[q.as_str()]), false) {
            Ok(a) => a,
            Err(exit) => return exit,
        };
        let found = resolved(&answer);
        if found.is_empty() {
            // `cli_print_or_fail … >/dev/null`: an error ends it here.
            if let Verdict::Error(err) = verdict(&answer, RESOLVE) {
                crate::out::stderr_line(&format!("Error: {err}"));
                return super::refused_or(&answer, Exit::Error);
            }
            let line = no_folder_line(&target, &answer, utf8_locale);
            let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), &line);
            return Exit::Error;
        }
        target = found;
    }
    client.project = target;
    match ws_request(ws, client, ENSURE, Call::RequestPost(&[]), false) {
        Ok(answer) => print_or_fail(&answer, ENSURE),
        Err(exit) => exit,
    }
}

/// `profile`, after the checks.
pub fn profile(ws: &mut Ws, client: &mut Client, project: &str, word: Option<String>) -> Exit {
    let target = target(word, project);
    let p = format!("project_path={}", quote(&target));
    client.project = target;
    match ws_request(
        ws,
        client,
        DISPLAY_NAME,
        Call::Request(&[p.as_str()]),
        false,
    ) {
        Ok(answer) => print_or_fail(&answer, DISPLAY_NAME),
        Err(exit) => exit,
    }
}

/// `update`, after the checks (`run` made sure `basename` is): `lookup`
/// for the agent's name, `client` for the POST.
pub fn update(
    ws: &mut Ws,
    lookup: &mut Client,
    client: &mut Client,
    project: &str,
    u: &Update,
) -> Exit {
    let answer = match ws_request(ws, lookup, DISPLAY_NAME, Call::Request(&[]), true) {
        Ok(a) => a,
        Err(exit) => return exit,
    };
    let name = match field(&answer, "display_name") {
        n if n.is_empty() => basename(project).unwrap_or_default(),
        n => n,
    };
    let pairs = [
        format!("agent={}", quote(&name)),
        format!("field={}", quote(&u.field)),
        format!("value={}", quote(&u.value)),
    ];
    let refs: Vec<&str> = pairs.iter().map(String::as_str).collect();
    match ws_request(ws, client, AGENT_UPDATE, Call::RequestPost(&refs), false) {
        Ok(answer) => print_or_fail(&answer, AGENT_UPDATE),
        Err(exit) => exit,
    }
}
