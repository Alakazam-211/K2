//! `k5 workspace launch [<path>]`, `k5 workspace profile [<path>]` and
//! `k5 workspace update --field <f> --value <v>`, as
//! `cmd_workspace_launch`, `cmd_workspace_profile` and
//! `cmd_workspace_update` (`cli/src/agent-context.sh`) do them.
//!
//! - `launch`: the first word after it (`${1:-$PROJECT}`: the workspace
//!   when there is none; any other word is skipped) is the target. When it
//!   is not a folder, `cli_request /cli/workspace/resolve q=<it>` asks the
//!   daemon for its path ([`look_up`]) and a path in the answer takes its
//!   place. Then `cli_request_post /cli/workspace/ensure-canonical-session
//!   project=<target>` ([`direct`]).
//! - `profile`: `cli_request /cli/workspace/agent-display-name
//!   project=<target>`, the target as for `launch` ([`direct`]).
//! - `update`: the words ([`parse_update`]); a projects DB column as the
//!   field is three lines and exit 2. Then the agent's name from
//!   `cli_request /cli/workspace/agent-display-name` ([`look_up`]), or the
//!   workspace folder's name; then `cli_request_post /cli/agent/update
//!   agent=<name>&field=<f>&value=<v>` ([`direct`]).
//!
//! Every call goes over TCP (the shell calls them directly). A lookup runs
//! as `$(cli_request … 2>/dev/null | python3 … 2>/dev/null)`, in a
//! subshell: its own client (`lookup`), so a passport it heals is not the
//! one the next call starts with, as in bash. It gives the
//! answer's `path` or `display_name` as Python prints it, nothing when it
//! has none (or is not JSON, or not an object); a lookup with no answer
//! hides its line and ends the command by SIGPIPE
//! ([`super::super::end_in_pipe`]); any other failure is its code, with
//! nothing printed (`set -e`). The last call prints the answer's bytes as
//! they came (NULs dropped), exit 0; no answer is the transport's line,
//! exit 1; any other failure is its code. A POST is sent once, as for
//! [`super::register`].
//!
//! The shell answers `update` with `--field` or `--value` as its last
//! word (it stops on `$2: unbound variable`, a line with its script's
//! path in it), and with a workspace whose path is not absolute or holds
//! a newline (`basename` there is not sure).

use super::super::{end_in_pipe, get, piped_body, s, without_nuls};
use super::Stop;
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error};

/// `launch`'s lookup.
pub const RESOLVE: &str = "/cli/workspace/resolve";
/// `launch`'s call.
pub const ENSURE: &str = "/cli/workspace/ensure-canonical-session";
/// `profile`'s call, and `update`'s lookup.
pub const DISPLAY_NAME: &str = "/cli/workspace/agent-display-name";
/// `update`'s call.
pub const AGENT_UPDATE: &str = "/cli/agent/update";
/// `update` without a field or a value.
pub const UPDATE_USAGE: &str = "Usage: k2 workspace update --field <f> --value <v>";

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
        "'k2 workspace update' only edits the agent's AGENT.md; it would not change the real setting.".to_string(),
        "Use: k2 agent set <name> [--mode <m>] [--enabled <bool>] [--name <new>]  (see 'k2 agent set --help')".to_string(),
    ]
}

/// `update`'s field and value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub field: String,
    pub value: String,
}

/// The words after `update`, as its loop reads them. `None`: `--field` or
/// `--value` as the last word (the shell's).
pub fn parse_update(words: &[&str]) -> Option<Result<Update, Stop>> {
    let (mut field, mut value) = (String::new(), String::new());
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--field" | "--value" => {
                let v = words.get(i + 1)?;
                if *w == "--field" {
                    field = (*v).to_string();
                } else {
                    value = (*v).to_string();
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    if field.is_empty() || value.is_empty() {
        return Some(Err(Stop {
            lines: vec![UPDATE_USAGE.to_string()],
            exit: Exit::Error,
        }));
    }
    if DB_COLUMNS.contains(&field.as_str()) {
        return Some(Err(Stop {
            lines: db_column_lines(&field),
            exit: Exit::Usage,
        }));
    }
    Some(Ok(Update { field, value }))
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

/// The lookup's Python on what it read: `print(json.load(sys.stdin).get(key, ""))`
/// under a bare `except`, as `$(…)` holds it (NULs and trailing newlines
/// gone).
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

/// A lookup (see the module's notes): what it found, or the exit that ends
/// the command.
pub fn look_up(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
    key: &str,
) -> Result<String, Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::Request(args)) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(field(&piped_body(&r), key))
        }
        // `_k2_end_command` from inside the pipe; its line went to
        // /dev/null.
        Err(Error::NoAnswer(_)) => end_in_pipe(),
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// A call whose answer the shell prints as it came.
pub fn direct(client: &mut Client, closing: &mut Closing, endpoint: &str, call: Call<'_>) -> Exit {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    }
}

/// `${1:-$PROJECT}`.
fn target(word: Option<String>, project: &str) -> String {
    word.filter(|w| !w.is_empty())
        .unwrap_or_else(|| project.to_string())
}

/// `[[ -d "$target" ]]`.
fn is_dir(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// `launch`, after the checks.
pub fn launch(
    client: &mut Client,
    lookup: &mut Client,
    project: &str,
    word: Option<String>,
) -> Exit {
    let mut closing = Closing::default();
    let mut target = target(word, project);
    if !is_dir(&target) {
        let q = format!("q={}", quote(&target));
        match look_up(lookup, &mut closing, RESOLVE, &[q.as_str()], "path") {
            Ok(found) if !found.is_empty() => target = found,
            Ok(_) => {}
            Err(exit) => {
                closing.flush(client, exit);
                return exit;
            }
        }
    }
    let p = format!("project={}", quote(&target));
    let exit = direct(
        client,
        &mut closing,
        ENSURE,
        Call::RequestPost(&[p.as_str()]),
    );
    closing.flush(client, exit);
    exit
}

/// `profile`, after the checks.
pub fn profile(client: &mut Client, project: &str, word: Option<String>) -> Exit {
    let mut closing = Closing::default();
    let p = format!("project={}", quote(&target(word, project)));
    let exit = direct(
        client,
        &mut closing,
        DISPLAY_NAME,
        Call::Request(&[p.as_str()]),
    );
    closing.flush(client, exit);
    exit
}

/// `update`, after the checks (`run` made sure `basename` is).
pub fn update(client: &mut Client, lookup: &mut Client, project: &str, u: &Update) -> Exit {
    let mut closing = Closing::default();
    let name = match look_up(lookup, &mut closing, DISPLAY_NAME, &[], "display_name") {
        Ok(n) if !n.is_empty() => n,
        Ok(_) => basename(project).unwrap_or_default(),
        Err(exit) => {
            closing.flush(client, exit);
            return exit;
        }
    };
    let pairs = [
        format!("agent={}", quote(&name)),
        format!("field={}", quote(&u.field)),
        format!("value={}", quote(&u.value)),
    ];
    let refs: Vec<&str> = pairs.iter().map(String::as_str).collect();
    let exit = direct(client, &mut closing, AGENT_UPDATE, Call::RequestPost(&refs));
    closing.flush(client, exit);
    exit
}
