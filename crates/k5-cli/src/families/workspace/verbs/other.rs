//! `workspace triage`, `remove`, `resources` and `preview`:
//!
//! - `triage` (`cli/src/agent-context.sh`): no words; in a session it
//!   needs the passport (else its line, exit 1, nothing sent); then
//!   `cli_send cli_request /cli/inbox/list | format_inbox`
//!   ([`crate::families::inbox::format_inbox`]). It sends the session's
//!   passport, never the owner token.
//! - `remove <path|name> [--mode m]` (`cli/src/wiki.sh`): a folder as
//!   `cd` names it, then the lookup with its errors shown (no answer is
//!   the line and exit 1; any other failure, or no path, is "not found",
//!   exit 4), then with `--apps <way>` the Apps' release ([`release`]),
//!   then `cli_request_post /cli/workspace/remove` and
//!   `cli_print_or_fail` ([`super::trust::print_plain`]).
//! - `resources list|add|remove [--workspace <ws>] [--json] [file]`
//!   (`cli/src/agent-context.sh`): `cli_send cli_request` (list) or
//!   `cli_send cli_post_form` (add, remove; the file made absolute as
//!   Python's `os.path.abspath` does), held in `$(…)`, then
//!   `_workspace_resources_emit` ([`emit`]).
//! - `preview [<path>]` (`cli/src/wiki.sh`): what adding the folder would
//!   do to the files in it, from the folder alone; nothing is sent.
//!
//! The shell answers `remove … --mode` as the last word, `remove --apps
//! <way>` in a session with no passport (its release tries the cell
//! socket), and a path the binary can't resolve as bash's `cd` would.

use super::{stop, Ask as Top, Here, Sent};
use crate::exit::Exit;
use crate::families::app::call::{self, Cj};
use crate::families::workspace::register::{self, Verb as RegisterVerb};
use crate::families::{inbox, pipefail, Shown};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::transport::encode::quote;
use crate::transport::{permission, Call, Channel, Error, Response};
use k2_client::brand::PRODUCT_NAME;
use k2_client::cli_name;
use std::path::Path;

pub const LIST: &str = "/cli/inbox/list";
pub const REMOVE: &str = "/cli/workspace/remove";
pub const RESOURCES: &str = "/cli/workspace/resources";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Triage,
    Remove(Remove),
    Resources(Resources),
    /// The folder as `cd` names it.
    Preview(String),
}

impl Ask {
    pub fn sends_on_socket(&self) -> bool {
        match self {
            Ask::Resources(_) => true,
            Ask::Remove(r) => !r.apps.is_empty(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remove {
    /// The path or name as typed.
    pub token: String,
    /// As `cd` names it.
    pub path: String,
    pub mode: String,
    /// `--apps`'s way (`handover:<ws>`, `uninstall`, `park`); empty: no
    /// release.
    pub apps: String,
    /// `--drop-access`.
    pub drop_access: bool,
    /// `--drop-access` without `--apps uninstall`: its line, after the
    /// lookup.
    pub drop_misused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    pub sub: String,
    /// `--workspace`'s value (empty: `$PROJECT`).
    pub ws: String,
    pub json: bool,
    /// The file (`add`, `remove`), made absolute; empty for none.
    pub file: String,
    /// A file was given (for `list`'s usage line).
    pub has_file: bool,
}

const RES_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace resources <list|add|remove> [--workspace <path>] [--json] [file]"
);

/// The words after the verb.
pub fn parse(verb: &str, words: &[&str], here: &Here) -> Top {
    match verb {
        "triage" if !words.is_empty() => stop(
            &[
                concat!("Usage: ", cli_name!(), " workspace triage"),
                concat!(
                    "This workspace's inbox list (same as ",
                    cli_name!(),
                    " inbox list). Extra arguments are not accepted."
                ),
            ],
            Exit::Usage,
        ),
        "triage" => Top::Other(Ask::Triage),
        "remove" => remove(words, here),
        "resources" => resources(words),
        _ => {
            let target = words.first().copied().unwrap_or("");
            let target = if target.is_empty() {
                here.pwd.as_str()
            } else {
                target
            };
            match register::path(RegisterVerb::Open, target, &here.pwd, cdpath()) {
                Some(p) => Top::Other(Ask::Preview(p)),
                None => Top::Shell,
            }
        }
    }
}

fn cdpath() -> bool {
    std::env::var_os("CDPATH").is_some()
}

fn remove(words: &[&str], here: &Here) -> Top {
    let (mut target, mut mode, mut apps) = (String::new(), String::new(), String::new());
    let mut drop_access = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--mode" | "--apps" => {
                // `--mode` last: `mode="$2"` stops on `$2: unbound
                // variable`, a line with the script's path in it (the
                // shell's). `--apps` last: `apps_way="${2:-}"; shift 2`,
                // whose `shift 2` fails, so `set -e` ends the command with
                // 1 and no line.
                let Some(v) = words.get(i + 1) else {
                    return if w == "--mode" {
                        Top::Shell
                    } else {
                        stop(&[], Exit::Error)
                    };
                };
                if w == "--mode" {
                    mode = (*v).to_string();
                } else {
                    apps = (*v).to_string();
                }
                i += 2;
                continue;
            }
            "--drop-access" => drop_access = true,
            w if w.starts_with("--mode=") => mode = w["--mode=".len()..].to_string(),
            w if w.starts_with("--apps=") => apps = w["--apps=".len()..].to_string(),
            w if w.starts_with('-') => return stop(&[&format!("Unknown flag: {w}")], Exit::Error),
            w => {
                if target.is_empty() {
                    target = w.to_string();
                }
            }
        }
        i += 1;
    }
    if target.is_empty() {
        return stop(&REMOVE_USAGE, Exit::Error);
    }
    let Some(path) = register::path(RegisterVerb::Open, &target, &here.pwd, cdpath()) else {
        return Top::Shell;
    };
    Top::Other(Ask::Remove(Remove {
        token: target,
        path,
        mode,
        drop_misused: drop_access && apps != "uninstall",
        apps,
        drop_access,
    }))
}

const REMOVE_USAGE: [&str; 20] = [
    concat!(
        "Usage: ",
        cli_name!(),
        " workspace remove <path> [--mode keep-current|restore-original]"
    ),
    "                           [--apps handover:<ws>|uninstall|park [--drop-access]]",
    "",
    "  --mode keep-current      Freeze the current SKILL.md body into each harness file so",
    concat!(
        "                           every CLI LLM keeps working after ",
        k2_client::product_name!(),
        " disconnects."
    ),
    concat!(
        "  --mode restore-original  Restore pre-",
        k2_client::product_name!(),
        " files from .k2/migration/; files ",
        k2_client::product_name!()
    ),
    "                           created fresh are removed. .k2/ stays intact for reconnect.",
    "  (no mode)                DB-only deregister; filesystem untouched (symlinks remain).",
    "  --apps handover:<ws>|uninstall|park",
    "                           First release the Apps this workspace maintains:",
    "                           hand them to another workspace, uninstall them, or",
    "                           park them (stopped, held for the owner). Without",
    "                           --apps, a workspace with Apps is refused (exit 1).",
    "                           All or nothing: if the workspace can't be removed, or",
    "                           an App would lose people's access (uninstall), no App",
    "                           is released and the reasons are listed, each App with",
    "                           its counts (exit 3).",
    "  --drop-access            With --apps uninstall: uninstall even Apps whose users",
    "                           would lose their logins, grants, shared secrets or",
    "                           chat Spaces. With any other --apps: exit 2.",
];

fn resources(words: &[&str]) -> Top {
    let sub = words.first().copied().unwrap_or("").to_string();
    let rest = words.get(1..).unwrap_or(&[]);
    let (mut json, mut ws, mut file) = (false, String::new(), String::new());
    let mut i = 0;
    let mut after: &[&str] = &[];
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--json" => json = true,
            "--workspace" | "--project" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    let name = if sub.is_empty() {
                        "<list|add|remove>"
                    } else {
                        sub.as_str()
                    };
                    return stop(
                        &[&format!(
                            concat!(
                                "Usage: ",
                                cli_name!(),
                                " workspace resources {} --workspace <path>"
                            ),
                            name
                        )],
                        Exit::Usage,
                    );
                }
                ws = v.to_string();
                i += 2;
                continue;
            }
            "--" => {
                after = &rest[i + 1..];
                break;
            }
            w if w.starts_with('-') => {
                return stop(&[&format!("Unknown flag: {w}"), RES_USAGE], Exit::Usage)
            }
            w => {
                if !file.is_empty() {
                    return stop(&[RES_USAGE], Exit::Usage);
                }
                file = w.to_string();
            }
        }
        i += 1;
    }
    if after.len() > 1 || (after.len() == 1 && !file.is_empty()) {
        return stop(&[RES_USAGE], Exit::Usage);
    }
    if let Some(f) = after.first() {
        file = (*f).to_string();
    }
    match sub.as_str() {
        "list" | "add" | "remove" => {}
        "" => return stop(&[RES_USAGE], Exit::Usage),
        _ => {
            return stop(
                &[
                    &format!("Unknown 'workspace resources' subcommand: {sub}"),
                    RES_USAGE,
                ],
                Exit::Usage,
            )
        }
    }
    let has_file = !file.is_empty();
    let file = if has_file && sub != "list" {
        match abspath(&file) {
            Some(f) => f,
            None => return Top::Shell,
        }
    } else {
        String::new()
    };
    Top::Other(Ask::Resources(Resources {
        sub,
        ws,
        json,
        file,
        has_file,
    }))
}

/// Python's `os.path.abspath(p)`: joined to the working folder (as the
/// OS names it) when relative, then `normpath`.
pub fn abspath(p: &str) -> Option<String> {
    let full = if p.starts_with('/') {
        p.to_string()
    } else {
        let cwd = std::env::current_dir().ok()?;
        format!("{}/{p}", cwd.to_str()?)
    };
    Some(normpath(&full))
}

/// Python's `posixpath.normpath` for a path that starts with `/`.
pub fn normpath(p: &str) -> String {
    let slashes = if p.starts_with("//") && !p.starts_with("///") {
        2
    } else {
        1
    };
    let mut comps: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if comps.last().is_some_and(|l| *l != "..") {
                    comps.pop();
                }
            }
            c => comps.push(c),
        }
    }
    format!("{}{}", "/".repeat(slashes), comps.join("/"))
}

/// The verb's requests and lines.
pub fn send(w: &mut Sent, a: &Ask) -> Exit {
    match a {
        Ask::Triage => {
            if w.settings.session && w.client.passport().is_none() {
                out::stderr_line(concat!(
                    "Error: ",
                    cli_name!(),
                    " workspace triage in an agent's session needs the session's passport, and this session has none."
                ));
                return Exit::Error;
            }
            let r = w.client.send(LIST, Call::Request(&[]));
            let piped = w.piped(r);
            let shown = Shown::out(inbox::format_inbox(&piped.body));
            pipefail(piped.exit, shown.print())
        }
        Ask::Remove(r) => remove_send(w, r),
        Ask::Resources(r) => resources_send(w, r),
        Ask::Preview(p) => preview(p),
    }
}

fn remove_send(w: &mut Sent, rm: &Remove) -> Exit {
    let q = format!("q={}", quote(&rm.path));
    let resolved = match w.request(super::RESOLVE, &[&q]) {
        Ok(r) => {
            w.note(&r);
            super::resolved_of(&crate::families::piped_body(&r))
        }
        Err(
            e @ (crate::transport::Error::NoAnswer(_)
            | crate::transport::Error::MaybeApplied { .. }),
        ) => return w.failed_in_pipe(&e, false),
        Err(_) => String::new(),
    };
    if resolved.is_empty() {
        out::stderr_line(&format!(
            "Error: workspace '{}' not found (by name or path).",
            rm.token
        ));
        out::stderr_line(concat!(
            "Run '",
            cli_name!(),
            " workspace list' for registered workspaces."
        ));
        return Exit::NotFound;
    }
    if rm.drop_misused {
        out::stderr_line("usage: --drop-access goes with --apps uninstall");
        return Exit::Usage;
    }
    if !rm.apps.is_empty() {
        if let Err(exit) = release(w, &resolved, rm) {
            return exit;
        }
    }
    let mode = rm.mode.as_str();
    let p = format!("path={}", quote(&resolved));
    let m = format!("mode={}", quote(mode));
    let args: Vec<&str> = if mode.is_empty() {
        vec![p.as_str()]
    } else {
        vec![p.as_str(), m.as_str()]
    };
    let r = w.request_post(REMOVE, &args);
    match w.held(r, false) {
        Ok(resp) => super::trust::print_plain(&resp),
        Err(exit) => exit,
    }
}

/// `/cli/app/release`, the route `remove --apps` posts first.
pub const RELEASE: &str = "/cli/app/release";

/// `_app_post_confirming /cli/app/release "Give this workspace's Apps to
/// ${apps_way#handover:}" K2_CJ_workspace=<path> K2_CJ_apps=<way>
/// K2_CJ_dropAccess_b=<0|1> K2_CJ_forRemoval_b=1`, then `[ "$_CP_STATUS"
/// = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"`: the POST
/// (`_compute_send`), and once more with a confirm token when the daemon
/// answers 403 `vault_confirm_required` with an action and a secret id.
/// Any status but 200 is `_compute_fail`'s line with no refusal registry
/// (this is no App command), exit 2 for a 400 or 413, else 3. A 202 too:
/// the shell takes only 200 here.
fn release(w: &mut Sent, path: &str, rm: &Remove) -> Result<(), Exit> {
    let drop = if rm.drop_access { "1" } else { "0" };
    let body = |tok: &str| {
        Cj::new()
            .s("confirmToken", tok)
            .s("workspace", path)
            .s("apps", &rm.apps)
            .b("dropAccess", drop)
            .b("forRemoval", "1")
            .body()
    };
    let mut s = compute_post(w, &body(""))?;
    if s.status == "403" {
        if let Some((action, sid)) = confirm_asked(&s.body) {
            let label = format!(
                "Give this workspace's Apps to {}",
                rm.apps.strip_prefix("handover:").unwrap_or(&rm.apps)
            );
            let passport = w.client.passport().is_some();
            let sock = crate::confirm::sock_path(&w.settings.home);
            let mut c = crate::confirm::Confirm {
                client: &mut *w.client,
                sock: &sock,
                closing: &mut *w.closing,
                passport,
            };
            let Some(tok) = crate::confirm::confirm_token(&mut c, &action, &sid, &label) else {
                return Err(Exit::Refused);
            };
            s = compute_post(w, &body(&tok))?;
        }
    }
    if s.ok() {
        return Ok(());
    }
    // A body that is not UTF-8 makes `_compute_fail`'s Python fail (its
    // stderr thrown away): the no-answer line.
    let readable: &[u8] = if std::str::from_utf8(&s.body).is_ok() {
        &s.body
    } else {
        b""
    };
    out::stderr_line(&crate::spec::run::fail_line(&s.status, readable));
    Err(call::fail_exit(&s.status))
}

/// `raw="$(_compute_send POST <RELEASE> <body>)"; _compute_split "$raw"`,
/// as [`call::Ctx::send`] does it for the app family, on this verb's
/// client. `Err`: the command ended (a POST that may have been applied
/// on the cell socket, a session's owner token, no answer: the line, exit
/// 1).
fn compute_post(w: &mut Sent, body: &str) -> Result<call::Split, Exit> {
    let result = w.client.compute_send(true, RELEASE, body, false);
    let refused = w.client.refused_before_answer();
    let raw = match &result {
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            return Err(Exit::Error);
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Err(Exit::Error);
        }
        other => call::held(other, refused),
    };
    let split = call::split(&raw);
    let noted = Response {
        status: 0,
        body: split.body.clone(),
        channel: result.as_ref().map_or(Channel::Tcp, |r| r.channel),
        permission: permission::note(&split.body),
        server_version: None,
    };
    w.note(&noted);
    Ok(split)
}

/// `_app_post_confirming`'s Python on a 403: the error's `action` and
/// `secretId` when its `code` is `vault_confirm_required` and both are
/// true, as `sed -n 1p` and `sed -n 2p` read the two lines it prints.
fn confirm_asked(body: &[u8]) -> Option<(String, String)> {
    let Ok(Value::Object(pairs)) = json::parse(std::str::from_utf8(body).ok()?) else {
        return None;
    };
    let Some(Value::Object(e)) = crate::families::get(&pairs, "error") else {
        return None;
    };
    let field = |k: &str| crate::families::get(e, k);
    if field("code") != Some(&crate::families::s("vault_confirm_required")) {
        return None;
    }
    let (action, sid) = (field("action")?, field("secretId")?);
    if !py::truthy(action) || !py::truthy(sid) {
        return None;
    }
    for v in [action, sid] {
        if let Value::Str(t) = v {
            // `print` can't write half a surrogate pair: it raises there.
            t.as_str()?;
        }
    }
    let printed = format!("{}\n{}", py::str_of(action), py::str_of(sid));
    // `$(…)` drops NULs.
    let printed = printed.replace('\0', "");
    let mut lines = printed.split('\n');
    let first = lines.next().unwrap_or("").to_string();
    let second = lines.next().unwrap_or("").to_string();
    Some((first, second))
}

fn resources_send(w: &mut Sent, r: &Resources) -> Exit {
    let ws = if r.ws.is_empty() {
        w.settings.project.clone()
    } else {
        r.ws.clone()
    };
    if ws.is_empty() {
        out::stderr_line("workspace: missing workspace (name | path | UUID)");
        return Exit::Error;
    }
    let wsp = format!("workspace={}", quote(&ws));
    let result = if r.sub == "list" {
        if r.has_file {
            out::stderr_line(concat!(
                "Usage: ",
                cli_name!(),
                " workspace resources list [--workspace <path>] [--json]"
            ));
            return Exit::Usage;
        }
        w.client.send(RESOURCES, Call::Request(&[&wsp]))
    } else {
        if !r.has_file {
            out::stderr_line(&format!(
                concat!(
                    "Usage: ",
                    cli_name!(),
                    " workspace resources {} <file> [--workspace <path>] [--json]"
                ),
                r.sub
            ));
            return Exit::Usage;
        }
        let p = format!("path={}", quote(&r.file));
        let endpoint = format!("{RESOURCES}/{}", r.sub);
        w.client.send(&endpoint, Call::PostForm(&[&wsp, &p]))
    };
    let raw = match w.held(result, false) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(exit) => return exit,
    };
    emit(&raw, &r.sub, r.json).print()
}

/// `_workspace_resources_emit` on what `$(…)` held.
pub fn emit(raw: &str, mode: &str, want_json: bool) -> Shown {
    let mut out = String::new();
    let fail = |out: String, err: String| Shown {
        stdout: out,
        stderr: err,
        exit: Exit::Error,
    };
    let data = match json::parse(raw) {
        Ok(d) => d,
        Err(_) => {
            return fail(
                out,
                format!(
                    "Error: daemon returned an unparseable response: {}\n",
                    super::repr_head(raw, 300)
                ),
            )
        }
    };
    let pairs = match &data {
        Value::Object(p) if crate::families::get(p, "ok") == Some(&Value::Bool(true)) => p,
        _ => {
            let err = match &data {
                Value::Object(p) => crate::families::get(p, "error"),
                _ => None,
            };
            let line = match err {
                Some(Value::Object(ep)) => {
                    let code = crate::families::remind::shared::or(
                        crate::families::get(ep, "code"),
                        crate::families::s("error"),
                    );
                    let hint =
                        crate::families::remind::shared::or(crate::families::get(ep, "hint"), code);
                    format!("{}\n", py::str_of(&hint))
                }
                _ => format!("Error: {}\n", super::head(raw, 300)),
            };
            return fail(out, line);
        }
    };
    if want_json {
        out.push_str(raw);
        if !raw.ends_with('\n') {
            out.push('\n');
        }
        return Shown::out(out);
    }
    let str_field = |o: &[(json::Text, Value)], k: &str| match crate::families::get(o, k) {
        Some(v @ Value::Str(_)) => py::str_of(v),
        _ => String::new(),
    };
    if mode == "list" {
        let Some(Value::Array(docs)) = crate::families::get(pairs, "docs") else {
            return fail(out, "Error: list response missing docs\n".into());
        };
        for row in docs {
            let Value::Object(row) = row else {
                return fail(out, "Error: list row is not an object\n".into());
            };
            out.push_str(&format!(
                "{}\t{}\n",
                str_field(row, "fileName"),
                str_field(row, "filePath")
            ));
        }
    } else {
        let path = str_field(pairs, "filePath");
        if path.is_empty() {
            return fail(out, "Error: response missing filePath\n".into());
        }
        out.push_str(&path);
        out.push('\n');
    }
    Shown::out(out)
}

/// `cmd_workspace_preview`: from the folder alone.
fn preview(p: &str) -> Exit {
    let mut o = String::new();
    let mut line = |s: &str| {
        o.push_str(s);
        o.push('\n');
    };
    let at = |rel: &str| format!("{p}/{rel}");
    let file = |rel: &str| Path::new(&at(rel)).is_file();
    let link =
        |rel: &str| std::fs::symlink_metadata(at(rel)).is_ok_and(|m| m.file_type().is_symlink());
    let holds = |rel: &str, what: &[u8]| {
        std::fs::read(at(rel)).is_ok_and(|b| b.windows(what.len()).any(|x| x == what))
    };
    line(&format!(
        "Preview: what {PRODUCT_NAME} will do if you add {p}"
    ));
    line("");
    line(&format!(
        "Collision-prone files {PRODUCT_NAME} takes over (archive → import body into SKILL.md → symlink):"
    ));
    let mut any = false;
    for rel in [
        "CLAUDE.md",
        "GEMINI.md",
        "AGENT.md",
        ".goosehints",
        ".cursor/rules/k2so.mdc",
    ] {
        if file(rel) && !link(rel) {
            if holds(rel, b"k2so_generated: true") {
                // Prose ("made by K2"), not a token: the product's name
                // and the word after it, as the shell prints them.
                line(&format!(
                    "  {}  {}-generated (will refresh in place)",
                    crate::families::ljust(rel, 30),
                    PRODUCT_NAME
                ));
            } else {
                let size =
                    std::fs::metadata(at(rel)).map_or(String::new(), |m| m.len().to_string());
                line(&format!(
                    "  {}  {size} bytes → will archive + import",
                    crate::families::ljust(rel, 30)
                ));
                any = true;
            }
        }
    }
    if file(".aider.conf.yml") && !link(".aider.conf.yml") {
        if holds(".aider.conf.yml", b"SKILL.md") {
            line("  .aider.conf.yml               already references SKILL.md (no change)");
        } else {
            line("  .aider.conf.yml               will archive + merge SKILL.md into read: list");
        }
        any = true;
    }
    if !any {
        line("  (none — workspace is clean for first-time setup)");
    }
    line("");
    line(&format!(
        "Files {PRODUCT_NAME} will create fresh (no pre-existing file):"
    ));
    for rel in [
        "SKILL.md",
        "CLAUDE.md",
        "GEMINI.md",
        "AGENT.md",
        ".goosehints",
        ".cursor/rules/k2so.mdc",
        ".aider.conf.yml",
        ".opencode/agent/k2so.md",
        ".pi/skills/k2so/SKILL.md",
    ] {
        if !Path::new(&at(rel)).exists() {
            line(&format!("  {rel}"));
        }
    }
    line("");
    line(&format!(
        "Marker-injected (preserves your existing content, {PRODUCT_NAME} adds its own block):"
    ));
    for rel in ["AGENTS.md", ".github/copilot-instructions.md"] {
        if file(rel) {
            line(&format!(
                "  {rel}  (exists — {PRODUCT_NAME} block inserted between markers)"
            ));
        } else {
            line(&format!("  {rel}  (will create)"));
        }
    }
    line("");
    line("Nothing is destroyed — archives land in .k2/migration/ and can be restored via");
    line(&format!(
        concat!(
            "  ",
            cli_name!(),
            " workspace remove {} --mode restore-original"
        ),
        p
    ));
    match out::write_flushed(&mut std::io::stdout().lock(), o.as_bytes()) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}
