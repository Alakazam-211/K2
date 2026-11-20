//! `k5 compute` and its read verbs, as the shell CLI's `cmd_compute`
//! (`cli/src/compute-command.sh`) and its helpers answer them:
//!
//! - `nodes` (`ls`), `info <node>`, `queue <node>`, `jobs [filters]`,
//!   `grants [--node N] [--workspace WS]`: one `_compute_call` GET, then the
//!   answer as it came (`--json`) or `_compute_render <verb>`
//!   ([`render`]).
//! - `get <job> [--full]` and `status <job>`: the same, a 404 is exit 4,
//!   and `--json` prints the answer's `result`.
//! - `status` with no job: every node at a glance (`cmd_compute_status`,
//!   [`status`]).
//! - `receipt <job>` (`cmd_compute_provenance`), `events --audit [--limit
//!   N]` (over TCP), `usage [--days N]` (over TCP), `stats <job>`
//!   (`_compute_text`).
//! - `logs <job>` ([`logs`]): the whole log, page by page; `logs --search
//!   TEXT`: one string across the workspace's kept logs.
//! - `rightsize` (`_compute_text`).
//! - `preview` and `placement` with no word or `status` (over TCP): the
//!   owner's switches as they are.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]).
//! 2. `compute` is an `id` tool: with `HOOK_SOCK` set, the stale-passport
//!    and require-passport lines, exit 3 ([`crate::prelude::id_tool`]).
//! 3. The verb's words ([`parse`]): its usage line and exit 2, or, where
//!    the shell's `shift 2` runs out of words (a value flag last), exit 1
//!    and nothing printed. Nothing is sent after either.
//! 4. One `_compute_call` ([`call`]): `_compute_send` over
//!    [`Client::compute_send`] (the cell socket first with a passport and
//!    a live socket, then TCP; `tcp` verbs TCP only), then `_compute_split`
//!    and, on any status but 200, `_compute_fail`'s line and exit (400 and
//!    413: 2; 404: 3, or 4 for `get`; else 3).
//! 5. The answer, printed the verb's way, then the EXIT trap's lines
//!    ([`Closing`]): a refusal's "Ask with:" line, the dead-passport line,
//!    the bulletin.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`), before
//! anything is printed or sent: every other verb (another lane's, or not
//! native yet), `compute` alone or `help`, a command line with `-h` or
//! `--help`, `events --max-wait` with a duration `_compute_secs` refuses, a
//! word that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable
//! exported, a port that is set but is no port curl takes, and a `python3`
//! the binary can't tell. The exit for a 404 is the verb's own:
//! `_compute_fail` takes it from its caller, never from the environment.

use super::{events, logs, pyrt, render, status};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{curl_port, get, raised, without_nuls, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::{self, Closing};
use crate::transport::encode::quote;
use crate::transport::{permission, Channel, Client, Error, Response};
use crate::{passport, python};
use k2_client::brand::PRODUCT_NAME;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The tool's name in the shell's policy.
pub const TOOL: &str = "compute";

/// The second words the read side answers.
pub const ANSWERS: &[&str] = &[
    "nodes",
    "ls",
    "info",
    "queue",
    "jobs",
    "get",
    "status",
    "grants",
    "receipt",
    "events",
    "stats",
    "usage",
    "logs",
    "preview",
    "placement",
    "rightsize",
];

/// The line `_compute_text` prints for a 200 with no `text`.
pub const NO_TEXT: &str = concat!("(this server sent no text: ", cli_name!(), " update)");

pub const INFO_USAGE: &str = concat!("usage: ", cli_name!(), " compute info <node>");
pub const QUEUE_USAGE: &str = concat!("usage: ", cli_name!(), " compute queue <node> [--json]");
pub const JOBS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute jobs [--node N] [--mine] [--state S] [--tier T] [--tag T] [--limit N] [--workspace WS] [--json]"
);
pub const TIER_USAGE: &str = "usage: --tier ci|priority|standard|batch";
pub const GET_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute get <job> [--full] [--json]"
);
pub const STATUS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute status [--json]   (",
    cli_name!(),
    " compute status <job> = get)"
);
pub const RECEIPT_USAGE: &str = concat!("usage: ", cli_name!(), " compute receipt <job>");
pub const PROVENANCE_OUT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute provenance <job> --out FILE"
);
pub const VERIFY_FILE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute verify <job | FILE.intoto.jsonl> --file PATH"
);
pub const VERIFY_KEY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute verify <job | FILE.intoto.jsonl> --owner-key FP"
);
pub const EVENTS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute events [--follow] [--mine] [--tag T] [--since CURSOR] [--limit N] [--max-wait D] [--json]  |  ",
    cli_name!(),
    " compute events --audit [--limit N]"
);
pub const STATS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute stats <job> [--json]  (a job id or its first 8 characters)"
);

pub const PREVIEW_USAGE: &str = concat!("usage: ", cli_name!(), " compute preview on|off|status");
pub const PLACEMENT_USAGE: &str =
    concat!("usage: ", cli_name!(), " compute placement v1|v2|status");

pub const RIGHTSIZE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute rightsize [--check NAME | --tag NAME | -- <command…>] [--days 14] [--workspace WS] [--json]  (one of the three)"
);
pub const DAYS_USAGE: &str = "usage: --days is a number of days (1-90)";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Nodes {
        json: bool,
    },
    Info {
        node: String,
        json: bool,
    },
    Queue {
        node: String,
        json: bool,
    },
    Jobs {
        query: String,
        json: bool,
    },
    Status {
        json: bool,
    },
    Get {
        job: String,
        full: bool,
        json: bool,
    },
    Grants {
        query: String,
        json: bool,
    },
    Receipt {
        job: String,
        json: bool,
    },
    EventsAudit {
        limit: String,
        json: bool,
    },
    Stats {
        job: String,
        json: bool,
    },
    Usage {
        days: String,
    },
    Logs(logs::Logs),
    LogsSearch(logs::Search),
    Events(events::Events),
    Preview,
    Placement,
    Rightsize {
        ws: String,
        check: String,
        tag: String,
        argv: Vec<String>,
        days: String,
        json: bool,
    },
}

/// Why the words stop the command before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The usage line, exit 2.
    Usage(String),
    /// `shift 2` with one word left: `set -e` ends the command with 1,
    /// nothing printed.
    Shift,
}

/// The words after `compute` (`words`). `None`: the shell's (see the
/// module's notes).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let (&sub, rest) = words.split_first()?;
    if matches!(sub, "" | "help" | "--help" | "-h")
        || rest.iter().any(|w| matches!(*w, "--help" | "-h"))
    {
        return None;
    }
    let json = rest.contains(&"--json");
    let usage = |u: &str| Some(Err(Stop::Usage(u.to_string())));
    let first = rest.first().copied().unwrap_or("");
    Some(Ok(match sub {
        "nodes" | "ls" => Ask::Nodes { json },
        "info" => {
            if first.is_empty() {
                return usage(INFO_USAGE);
            }
            Ask::Info {
                node: first.into(),
                json,
            }
        }
        "queue" => {
            if first.is_empty() || first.starts_with('-') {
                return usage(QUEUE_USAGE);
            }
            Ask::Queue {
                node: first.into(),
                json,
            }
        }
        "jobs" => match jobs_query(rest) {
            Ok(query) => Ask::Jobs { query, json },
            Err(stop) => return Some(Err(stop)),
        },
        "status" if first.is_empty() || first.starts_with('-') => {
            if rest.iter().any(|w| *w != "--json") {
                return usage(STATUS_USAGE);
            }
            Ask::Status { json }
        }
        "get" | "status" => {
            if first.is_empty() {
                return usage(GET_USAGE);
            }
            Ask::Get {
                job: first.into(),
                full: rest.contains(&"--full"),
                json,
            }
        }
        "grants" => {
            let mut q = String::new();
            let mut i = 0;
            while let Some(w) = rest.get(i) {
                match *w {
                    "--node" | "--workspace" => {
                        let Some(v) = rest.get(i + 1) else {
                            return Some(Err(Stop::Shift));
                        };
                        q.push_str(&format!("&{}={}", &w[2..], quote(v)));
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            Ask::Grants {
                query: q.strip_prefix('&').unwrap_or(&q).to_string(),
                json,
            }
        }
        "receipt" => match provenance_words(sub, rest) {
            Ok((target, json)) => {
                if target.is_empty() {
                    return usage(RECEIPT_USAGE);
                }
                Ask::Receipt { job: target, json }
            }
            Err(stop) => return Some(Err(stop)),
        },
        "events" => {
            let (mut audit, mut ev) = (false, events::Events::default());
            let mut max_wait = String::new();
            let mut i = 0;
            while let Some(w) = rest.get(i) {
                match *w {
                    "--audit" => audit = true,
                    "--follow" | "-f" => ev.follow = true,
                    "--mine" => ev.mine = true,
                    "--json" => {}
                    "--limit" | "--tag" | "--since" | "--max-wait" => {
                        let Some(v) = rest.get(i + 1) else {
                            return Some(Err(Stop::Shift));
                        };
                        let v = v.to_string();
                        match *w {
                            "--limit" => ev.limit = v,
                            "--tag" => ev.tag = v,
                            "--since" => ev.since = v,
                            _ => max_wait = v,
                        }
                        i += 1;
                    }
                    _ => return usage(EVENTS_USAGE),
                }
                i += 1;
            }
            if audit {
                Ask::EventsAudit {
                    limit: if ev.limit.is_empty() {
                        "50".into()
                    } else {
                        ev.limit
                    },
                    json,
                }
            } else {
                if !max_wait.is_empty() {
                    // A duration _compute_secs refuses ends in the shell's
                    // own arithmetic error: the shell's.
                    ev.max_wait = Some(events::secs(&max_wait)?);
                }
                ev.json = json;
                Ask::Events(ev)
            }
        }
        "stats" => {
            let (mut job, mut json) = (String::new(), false);
            for w in rest {
                match *w {
                    "--json" => json = true,
                    w if w.starts_with('-') => return usage(STATS_USAGE),
                    w => {
                        if !job.is_empty() {
                            return usage(STATS_USAGE);
                        }
                        job = w.to_string();
                    }
                }
            }
            let ok = (4..=64).contains(&job.len())
                && job
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !ok {
                return usage(STATS_USAGE);
            }
            Ask::Stats { job, json }
        }
        "usage" => {
            let days = if first == "--days" {
                rest.get(1)
                    .copied()
                    .filter(|d| !d.is_empty())
                    .unwrap_or("7")
            } else {
                "7"
            };
            Ask::Usage { days: days.into() }
        }
        "preview" => match first {
            "" | "status" => Ask::Preview,
            "on" | "off" => return None,
            _ => return usage(PREVIEW_USAGE),
        },
        "placement" => match first {
            "" | "status" => Ask::Placement,
            "v1" | "v2" => return None,
            _ => return usage(PLACEMENT_USAGE),
        },
        "rightsize" => {
            let (mut ws, mut check, mut tag, mut days, mut json) = (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                false,
            );
            let mut argv: Option<Vec<String>> = None;
            let mut i = 0;
            while let Some(w) = rest.get(i) {
                match *w {
                    "--" => {
                        argv = Some(rest[i + 1..].iter().map(|x| x.to_string()).collect());
                        break;
                    }
                    "--check" | "--tag" | "--days" | "--workspace" => {
                        let Some(v) = rest.get(i + 1) else {
                            return Some(Err(Stop::Shift));
                        };
                        match *w {
                            "--check" => check = v.to_string(),
                            "--tag" => tag = v.to_string(),
                            "--workspace" => ws = v.to_string(),
                            _ => {
                                days = v.strip_suffix('d').unwrap_or(v).to_string();
                                if days.is_empty() || !days.bytes().all(|b| b.is_ascii_digit()) {
                                    return usage(DAYS_USAGE);
                                }
                            }
                        }
                        i += 2;
                    }
                    "--json" => {
                        json = true;
                        i += 1;
                    }
                    _ => return usage(RIGHTSIZE_USAGE),
                }
            }
            let argv = argv.unwrap_or_default();
            let n = usize::from(!check.is_empty())
                + usize::from(!tag.is_empty())
                + usize::from(!argv.is_empty());
            if n != 1 {
                return usage(RIGHTSIZE_USAGE);
            }
            Ask::Rightsize {
                ws,
                check,
                tag,
                argv,
                days,
                json,
            }
        }
        "logs"
            if rest
                .iter()
                .any(|w| *w == "--search" || w.starts_with("--search=")) =>
        {
            match logs::parse_search(rest) {
                Ok(a) => Ask::LogsSearch(a),
                Err(stop) => return Some(Err(stop)),
            }
        }
        "logs" => match logs::parse(rest)? {
            Ok(l) => Ask::Logs(l),
            Err(stop) => return Some(Err(stop)),
        },
        _ => return None,
    }))
}

/// `jobs`' filters as the shell's loop reads them: the query (its first
/// `&` dropped), or where it stops.
fn jobs_query(rest: &[&str]) -> Result<String, Stop> {
    let mut q = String::new();
    let mut i = 0;
    while let Some(w) = rest.get(i) {
        match *w {
            "--node" | "--state" | "--limit" | "--workspace" | "--tag" => {
                let Some(v) = rest.get(i + 1) else {
                    return Err(Stop::Shift);
                };
                q.push_str(&format!("&{}={}", &w[2..], quote(v)));
                i += 2;
            }
            "--mine" => {
                q.push_str("&mine=1");
                i += 1;
            }
            "--tier" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                match v {
                    "ci" | "priority" | "normal" | "batch" => {
                        q.push_str(&format!("&tier={}", quote(v)))
                    }
                    "standard" => q.push_str("&tier=normal"),
                    _ => return Err(Stop::Usage(TIER_USAGE.into())),
                }
                i += 2;
            }
            "--json" => i += 1,
            _ => return Err(Stop::Usage(JOBS_USAGE.into())),
        }
    }
    Ok(q.strip_prefix('&').unwrap_or(&q).to_string())
}

/// `cmd_compute_provenance`'s loop for `verb`: the target and `--json`.
fn provenance_words(verb: &str, rest: &[&str]) -> Result<(String, bool), Stop> {
    let (mut target, mut json) = (String::new(), false);
    let mut i = 0;
    while let Some(w) = rest.get(i) {
        match *w {
            "--json" => {
                json = true;
                i += 1;
            }
            "--out" | "--file" | "--owner-key" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    return Err(Stop::Usage(
                        match *w {
                            "--out" => PROVENANCE_OUT_USAGE,
                            "--file" => VERIFY_FILE_USAGE,
                            _ => VERIFY_KEY_USAGE,
                        }
                        .into(),
                    ));
                }
                i += 2;
            }
            w if w.starts_with('-') => {
                return Err(Stop::Usage(format!(
                    concat!(
                        cli_name!(),
                        " compute {}: unknown flag {} (",
                        cli_name!(),
                        " compute --help)"
                    ),
                    verb, w
                )))
            }
            w => {
                if !target.is_empty() {
                    return Err(Stop::Usage(format!(
                        concat!(cli_name!(), " compute {}: one job at a time"),
                        verb
                    )));
                }
                target = w.to_string();
                i += 1;
            }
        }
    }
    Ok((target, json))
}

/// Runs `args` (the whole command line after `argv[0]`). `None`: the shell
/// CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    if words.first() != Some(&"compute") {
        return None;
    }
    let ask = parse(&words[1..])?;
    if !python::family_native(python::here()) {
        return None;
    }
    // logs runs the shell's mktemp: one it can't find or run as the shell
    // would is the shell's.
    if matches!(ask, Ok(Ask::Logs(_)))
        && crate::families::update::shell_program(&crate::offline::Env::current(), "mktemp")
            .is_none()
    {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    ask: Result<Ask, Stop>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match ask {
        Ok(a) => a,
        Err(Stop::Usage(line)) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
        Err(Stop::Shift) => return Exit::Error,
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let project = settings.project.clone();
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = answer(&mut client, &mut closing, &ask, &project);
    closing.flush(&mut client, exit);
    exit
}

/// `_compute_ws ""`: this checkout, outside a session (a passport's
/// workspace comes from the passport).
pub(super) fn ws(client: &Client, project: &str) -> String {
    if client.passport().is_some() {
        String::new()
    } else {
        project.to_string()
    }
}

fn answer(client: &mut Client, closing: &mut Closing, ask: &Ask, project: &str) -> Exit {
    let now = pyrt::now();
    match ask {
        Ask::Nodes { json } => {
            let w = ws(client, project);
            let q = if w.is_empty() {
                String::new()
            } else {
                format!("workspace={}", quote(&w))
            };
            match call(client, closing, "/cli/compute/nodes", &q, false, false) {
                Ok(body) => body_or(*json, &body, || render::render("nodes", &body, false, now)),
                Err(e) => e,
            }
        }
        Ask::Info { node, json } => {
            let q = format!("node={}", quote(node));
            match call(client, closing, "/cli/compute/nodes/info", &q, false, false) {
                Ok(body) => body_or(*json, &body, || render::render("info", &body, false, now)),
                Err(e) => e,
            }
        }
        Ask::Queue { node, json } => {
            let q = format!("node={}", quote(node));
            match call(client, closing, "/cli/compute/queue", &q, false, false) {
                Ok(body) => body_or(*json, &body, || render::render("queue", &body, false, now)),
                Err(e) => e,
            }
        }
        Ask::Jobs { query, json } => {
            match call(client, closing, "/cli/compute/jobs", query, false, false) {
                Ok(body) => body_or(*json, &body, || render::render("jobs", &body, false, now)),
                Err(e) => e,
            }
        }
        Ask::Status { json } => {
            let w = ws(client, project);
            let mut q = "detail=1".to_string();
            if !w.is_empty() {
                q.push_str(&format!("&workspace={}", quote(&w)));
            }
            match call(client, closing, "/cli/compute/nodes", &q, false, false) {
                Ok(body) => body_or(*json, &body, || status::render(&body, now)),
                Err(e) => e,
            }
        }
        Ask::Get { job, full, json } => {
            let q = format!("job={}", quote(job));
            match call(client, closing, "/cli/compute/jobs/get", &q, false, true) {
                Ok(body) if *json => result_json(&body).print(),
                Ok(body) => render::render("get", &body, *full, now).print(),
                Err(e) => e,
            }
        }
        Ask::Grants { query, json } => {
            match call(client, closing, "/cli/compute/grants", query, false, false) {
                Ok(body) => body_or(*json, &body, || render::render("grants", &body, false, now)),
                Err(e) => e,
            }
        }
        Ask::Receipt { job, json } => {
            let q = format!("job={}", quote(job));
            match call(
                client,
                closing,
                "/cli/compute/jobs/receipt",
                &q,
                false,
                false,
            ) {
                Ok(body) => body_or(*json, &body, || {
                    render::render("receipt", &body, false, now)
                }),
                Err(e) => e,
            }
        }
        Ask::EventsAudit { limit, json } => {
            let q = format!("limit={}", quote(limit));
            match call(client, closing, "/cli/compute/events", &q, true, false) {
                Ok(body) => body_or(*json, &body, || render::render("events", &body, false, now)),
                Err(e) => e,
            }
        }
        Ask::Stats { job, json } => {
            let q = format!("job={job}");
            match call(client, closing, "/cli/compute/stats", &q, false, false) {
                Ok(body) => body_or(*json, &body, || text_lines(&body)),
                Err(e) => e,
            }
        }
        Ask::Logs(l) => logs::run(client, closing, l),
        Ask::Events(e) => events::run(client, closing, e, project),
        Ask::LogsSearch(a) => {
            let w = if a.ws.is_empty() {
                ws(client, project)
            } else {
                a.ws.clone()
            };
            let q = logs::search_query(a, &w);
            match call(
                client,
                closing,
                "/cli/compute/logs/search",
                &q,
                false,
                false,
            ) {
                Ok(body) => body_or(a.json, &body, || logs::search_lines(&body, now)),
                Err(e) => e,
            }
        }
        Ask::Rightsize {
            ws: explicit,
            check,
            tag,
            argv,
            days,
            json,
        } => {
            let ws = if explicit.is_empty() {
                ws(client, project)
            } else {
                explicit.clone()
            };
            let argv_json = if argv.is_empty() {
                String::new()
            } else {
                let list = Value::Array(argv.iter().map(|a| pyrt::s(a)).collect());
                pyrt::dumps(&list, false, None).expect("a list of strings prints")
            };
            let q: Vec<String> = [
                ("workspace", ws.as_str()),
                ("check", check),
                ("tag", tag),
                ("argv", &argv_json),
                ("days", days),
            ]
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={}", quote(v).replace("%20", "+")))
            .collect();
            match call(
                client,
                closing,
                "/cli/compute/rightsize",
                &q.join("&"),
                false,
                false,
            ) {
                Ok(body) => body_or(*json, &body, || text_lines(&body)),
                Err(e) => e,
            }
        }
        Ask::Preview => match call(client, closing, "/cli/settings/get", "", true, false) {
            Ok(body) => preview(&body).print(),
            Err(e) => e,
        },
        Ask::Placement => match call(client, closing, "/cli/settings/get", "", true, false) {
            Ok(body) => placement(&body).print(),
            Err(e) => e,
        },
        Ask::Usage { days } => {
            let q = format!("days={}", quote(days));
            match call(client, closing, "/cli/compute/usage", &q, true, false) {
                Ok(body) => render::render("usage", &body, false, now).print(),
                Err(e) => e,
            }
        }
    }
}

/// `--json`: `printf '%s\n' "$_CP_BODY"`, exit 0; else `show`'s.
fn body_or(json: bool, body: &[u8], show: impl FnOnce() -> Shown) -> Exit {
    if !json {
        return show().print();
    }
    let mut text = body.to_vec();
    text.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &text) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `_compute_call METHOD PATH payload [tcp]`: the body of a 200, or the
/// command's exit after `_compute_fail`'s line (`get`: a 404 is 4).
pub fn call(
    client: &mut Client,
    closing: &mut Closing,
    path: &str,
    payload: &str,
    tcp: bool,
    get_404: bool,
) -> Result<Vec<u8>, Exit> {
    let r = client.compute_send(false, path, payload, tcp);
    if let Err(e @ Error::MaybeApplied { .. }) = &r {
        out::stderr_line(&e.to_string());
        return Err(Exit::Error);
    }
    let (status, body) = split(&raw(&r, client.refused_before_answer()));
    let synth = Response {
        status: status.parse().unwrap_or(0),
        permission: permission::note(&body),
        body: body.clone(),
        channel: Channel::Tcp,
        server_version: None,
    };
    closing.answer(&synth, client.passport().is_some());
    if status == "200" {
        return Ok(body);
    }
    out::stderr_line(&fail_line(&status, &body));
    Err(match status.as_str() {
        "400" | "413" => Exit::Usage,
        "404" if get_404 => Exit::NotFound,
        _ => Exit::Refused,
    })
}

/// What `raw="$(_compute_send …)"` holds: curl's `-w` line for each try
/// that nothing accepted (`refused`), then the answer and its status, or,
/// when the call failed, one more `-w` line and `_compute_send`'s own
/// `printf '\n000'`. `$(…)` drops the NULs; the status's digits end it, so
/// no newline is dropped.
pub fn raw(r: &Result<Response, Error>, refused: u32) -> Vec<u8> {
    let mut out = b"\n000".repeat(refused as usize);
    match r {
        Ok(r) => {
            out.extend(without_nuls(&r.body));
            out.extend(format!("\n{:03}", r.status).into_bytes());
        }
        Err(_) => out.extend(b"\n000\n000"),
    }
    out
}

/// `_compute_split`: the status after the last newline (`000` when
/// empty), the body before it (nothing when there is no newline).
pub fn split(raw: &[u8]) -> (String, Vec<u8>) {
    let (status, body) = match raw.iter().rposition(|b| *b == b'\n') {
        Some(i) => (&raw[i + 1..], raw[..i].to_vec()),
        None => (raw, Vec::new()),
    };
    let status = String::from_utf8_lossy(status).into_owned();
    if status.is_empty() {
        ("000".into(), body)
    } else {
        (status, body)
    }
}

/// `_compute_fail`'s line (not an App area: no refusal registry rows):
/// its Python's on the body (one JSON object of `error` and `hint`, or
/// `daemon_error: ` and the first 200 characters of a body that is not
/// JSON), `$(…)` dropping the trailing newlines; the no-answer object when
/// that is empty.
pub fn fail_line(status: &str, body: &[u8]) -> String {
    let mut line = String::new();
    if !body.is_empty() {
        let raw = String::from_utf8_lossy(body);
        line = match json::parse(&raw) {
            Err(_) => format!(
                "daemon_error: {}",
                raw.chars().take(200).collect::<String>()
            ),
            Ok(v) => {
                let e = match &v {
                    Value::Object(pairs) => get(pairs, "error"),
                    _ => None,
                };
                let message = match &v {
                    Value::Object(pairs) => get(pairs, "message"),
                    _ => None,
                };
                let s = |t: &str| Value::Str(Text::from(t));
                let (error, hint) = match (e, message) {
                    (Some(Value::Object(p)), _) => (
                        get(p, "code").cloned().unwrap_or_else(|| s("error")),
                        get(p, "hint").cloned().unwrap_or_else(|| s("")),
                    ),
                    (Some(e @ Value::Str(_)), Some(m @ Value::Str(_))) => (e.clone(), m.clone()),
                    (e, _) => (s("error"), s(&py::str_of(e.unwrap_or(&Value::Null)))),
                };
                let out = Value::Object(vec![
                    (Text::from("error"), error),
                    (Text::from("hint"), hint),
                ]);
                json::to_string(&out, &Options::default())
                    .expect("json.dumps with ensure_ascii writes any value json.loads gave")
            }
        };
        while line.ends_with('\n') {
            line.pop();
        }
    }
    if line.is_empty() {
        line = format!(
            "{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the {PRODUCT_NAME} daemon (status {status})\"}}"
        );
    }
    line
}

/// `get --json`: `print(json.dumps(json.load(sys.stdin).get("result"),
/// indent=2))`, all on the Python's line 1.
pub fn result_json(body: &[u8]) -> Shown {
    let cx = render::Cx::new(0);
    let v = match render::load(&cx, body, 1) {
        Ok(v) => v,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let r = match pyrt::get(&v, "result") {
        Ok(r) => r,
        Err(e) => return raised(String::new(), String::new(), &Raise::at(1, e)),
    };
    match pyrt::dumps(&r, false, Some(2)) {
        Ok(t) => Shown::out(t + "\n"),
        Err(e) => raised(String::new(), String::new(), &Raise::at(1, e)),
    }
}

/// `preview`'s (status) Python, all on its line 1.
pub fn preview(body: &[u8]) -> Shown {
    let cx = render::Cx::new(0);
    let r = render::load(&cx, body, 1).and_then(|v| {
        let on = pyrt::get(&v, "computePreview").map_err(|e| Raise::at(1, e))?;
        Ok(format!(
            "Compute nodes (preview): {} on this server\n",
            if pyrt::truthy(&on) { "on" } else { "off" }
        ))
    });
    match r {
        Ok(t) => Shown::out(t),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `placement`'s (status) Python.
pub fn placement(body: &[u8]) -> Shown {
    let cx = render::Cx::new(0);
    let r = render::load(&cx, body, 3).and_then(|v| {
        let p = pyrt::get(&v, "computePlacement").map_err(|e| Raise::at(3, e))?;
        let p = pyrt::or(p, pyrt::s("v2"));
        let what = render::lookup(
            &[
                (
                    "v2",
                    "load bands, a warm slot worth one band, bursts spread",
                ),
                ("v1", "the 0.46.5 order: warm slot first, then free CPU"),
            ],
            &p,
        )
        .map_err(|e| Raise::at(4, e))?
        .map_or_else(|| p.clone(), |w| pyrt::s(&w));
        let line = pyrt::pct("Compute placement: %s on this server (%s)", &[p, what])
            .map_err(|e| Raise::at(5, e))?;
        Ok(line + "\n")
    });
    match r {
        Ok(t) => Shown::out(t),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `_compute_text`'s text form: `print("\n".join(json.load(sys.stdin)
/// .get("text") or [NO_TEXT]))`, all on the Python's line 1.
pub fn text_lines(body: &[u8]) -> Shown {
    let cx = render::Cx::new(0);
    let v = match render::load(&cx, body, 1) {
        Ok(v) => v,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let text = match pyrt::get(&v, "text") {
        Ok(t) => pyrt::or(t, Value::Array(vec![pyrt::s(NO_TEXT)])),
        Err(e) => return raised(String::new(), String::new(), &Raise::at(1, e)),
    };
    match pyrt::join("\n", &text) {
        Ok(t) => Shown::out(t + "\n"),
        Err(e) => raised(String::new(), String::new(), &Raise::at(1, e)),
    }
}

#[cfg(test)]
mod tests;
