//! Folder trust, daemon side (prd-security-sweep-0464-v1 rev 2, SS9–SS11).
//!
//! The gate itself (scan, decisions, grants) is
//! `k2_core::cli_folder_trust::gate`. This module is what needs the
//! daemon:
//!
//! - **the card** ([`install`] registers the presenter): a one-tap action
//!   card `folder.trust` (Trust it / Always trust this folder / Don't
//!   trust) on the folder's own workspace Thread, else on the asking
//!   agent's Thread, else an owner ticket (a `/v1` hire or clone, a
//!   headless box with no pinned Chat). Only the owner answers
//!   (`require_action_card_person`, E2/A2). The card names who really
//!   asked, every line is escaped and the findings sit in a code block
//!   (reviews B#1, B#4); a tap is refused when the folder isn't what the
//!   card showed any more, and a fresh card is posted (B#2);
//! - **the spawn gate** ([`gate_spawn`]): every K2-started session in a
//!   folder, whatever the program (A-H2), asks the gate first; a held
//!   folder's spawn is queued for 24 h and retried once when the person
//!   trusts it, as the same caller ([`hold_spawn`], [`after_decision`]);
//!   heartbeats are skipped ([`heartbeat_hold`]);
//! - **the 409** ([`refusal_response`]): on the spawn routes only (B#6), a
//!   spawn error carrying the gate's random per-refusal token becomes
//!   `409 folder_trust_pending|folder_trust_denied` with the body that
//!   says how to decide;
//! - **the routes** ([`dispatch`]): `GET /cli/folder-trust/status` and
//!   `POST /cli/folder-trust/request` for registered workspace folders
//!   only (an agent: its own), `POST /cli/folder-trust/decide` (with the
//!   fingerprint the owner saw) and `POST /cli/folder-trust/reset` (the
//!   owner);
//! - **boot** ([`boot_pass`]): the 0.46.3 upgrade marks (one owner summary)
//!   and the re-assert sweep;
//! - "Don't trust", and a live change that holds a folder again, stop K2's
//!   running AI sessions in that scope (only that scope: B#5, A-M1).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use k2_core::cli_folder_trust::gate::{self, Actor, Ask, Asked, Decision, DecideError, Door, Outcome, Presenter, Refusal, Seen};
use k2_core::log_debug;

use crate::cli_response::CliResponse;

/// The action card id (`kd/action_cards.rs` `run`).
pub const CARD_ACTION: &str = "folder.trust";
pub const TRUST_IT: &str = "Trust it";
pub const ALWAYS: &str = "Always trust this folder";
pub const DONT: &str = "Don't trust";

pub const STATUS_PATH: &str = "/cli/folder-trust/status";
pub const REQUEST_PATH: &str = "/cli/folder-trust/request";
pub const DECIDE_PATH: &str = "/cli/folder-trust/decide";
pub const RESET_PATH: &str = "/cli/folder-trust/reset";
pub const FORGET_PATH: &str = "/cli/folder-trust/forget";

/// Findings listed on a card (the rest: `k2 workspace trust status`).
pub const CARD_LINES: usize = 5;
/// The 409 when the owner's decision names a folder state that changed.
pub const CHANGED: &str = "folder_trust_changed";
/// Held spawns older than this are dropped, not retried (review B#8).
pub const HELD_TTL: Duration = Duration::from_secs(24 * 3600);

// ── Install ──────────────────────────────────────────────────────────

/// Register the card presenter. Call once after the DB is open, before
/// any spawn (idempotent).
pub fn install() {
    if gate::has_presenter() {
        return;
    }
    gate::set_presenter(Presenter { present: Box::new(present), is_open: Box::new(gate::thread_card_or_ticket_open) });
}

// ── Where the card goes ──────────────────────────────────────────────

/// The registered workspace that holds `path`: an exact `projects.path`,
/// else the longest registered folder `path` sits under.
pub(crate) fn project_for_path(path: &str) -> Option<(String, String)> {
    let projects = k2_core::projects_ops::projects_list().ok()?;
    let p = Path::new(path);
    projects
        .into_iter()
        .filter(|pr| !pr.path.is_empty() && (pr.path == path || p.starts_with(&pr.path)))
        .max_by_key(|pr| pr.path.len())
        .map(|pr| (pr.id, pr.path))
}

/// The registered workspace whose folder IS `path` (canonical compare).
fn registered_exact(path: &str) -> Option<(String, String)> {
    let want = dunce::canonicalize(&Path::new(path)).ok()?;
    k2_core::projects_ops::projects_list()
        .ok()?
        .into_iter()
        .find(|pr| !pr.path.is_empty() && dunce::canonicalize(&Path::new(&pr.path)).ok().as_deref() == Some(want.as_path()))
        .map(|pr| (pr.id, pr.path))
}

fn pinned_conversation(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, project_id)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn agent_name_of(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| "An agent".to_string())
}

/// The workspace folder of a registered project id.
fn project_path_for_id(project_id: &str) -> Option<String> {
    k2_core::projects_ops::projects_list()
        .ok()?
        .into_iter()
        .find(|pr| pr.id == project_id && !pr.path.is_empty())
        .map(|pr| pr.path)
}

/// What the card says is being started: an AI tool's name, else the
/// program itself in code (review round 2 Low: a gated shell or script
/// is not "AI tools").
fn tool_display(tool: Option<&str>) -> String {
    let base = tool.map(|t| {
        let base = Path::new(t).file_name().and_then(|s| s.to_str()).unwrap_or(t).to_ascii_lowercase();
        base.trim_end_matches(".exe").to_string()
    });
    match base.as_deref() {
        Some("claude") => "Claude".into(),
        Some("codex") => "Codex".into(),
        Some("grok") => "Grok".into(),
        Some("gemini") => "Gemini".into(),
        Some("cursor-agent") => "Cursor".into(),
        Some(_) => md_code_inline(&tool.unwrap_or_default().chars().take(80).collect::<String>()),
        None => "a program".into(),
    }
}

// ── Who opened a tab (review round 2 Low) ────────────────────────────

thread_local! {
    /// The person whose request is being served (the owner token, a
    /// Connect login), for the card's "who asked".
    static PERSON: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with `person` named as the caller of any card it raises.
pub fn with_person<T>(person: Option<String>, f: impl FnOnce() -> T) -> T {
    let prev = PERSON.with(|p| p.replace(person));
    let out = f();
    PERSON.with(|p| *p.borrow_mut() = prev);
    out
}

fn request_person() -> Option<String> {
    PERSON.with(|p| p.borrow().clone())
}

// ── Markdown safety (review B#1) ─────────────────────────────────────

/// One line of plain text for a Thread card: control characters and
/// newlines become spaces, markdown punctuation is backslash-escaped, at
/// most `max` characters (an ellipsis says it was cut).
pub fn md_text(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.trim();
    let (cut, cutted) = if flat.chars().count() > max {
        (flat.chars().take(max.saturating_sub(1)).collect::<String>(), true)
    } else {
        (flat.to_string(), false)
    };
    let mut out = String::with_capacity(cut.len() + 8);
    for c in cut.chars() {
        if "\\`*_{}[]()<>#+-.!|~>".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    if cutted {
        out.push('…');
    }
    out
}

fn longest_backtick_run(s: &str) -> usize {
    let (mut best, mut run) = (0usize, 0usize);
    for c in s.chars() {
        if c == '`' {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Inline code that `s` can't break out of: a backtick fence longer than
/// any run inside, newlines flattened.
pub fn md_code_inline(s: &str) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let fence = "`".repeat(longest_backtick_run(&flat) + 1);
    format!("{fence} {flat} {fence}")
}

/// A fenced code block of `lines` (each flattened to one line) that the
/// content can't close: the fence is longer than any backtick run inside.
pub fn md_code_block(lines: &[String]) -> String {
    let flat: Vec<String> = lines.iter().map(|l| l.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()).collect();
    let longest = flat.iter().map(|l| longest_backtick_run(l)).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}text\n{}\n{fence}", flat.join("\n"))
}

/// How much a finding matters on a card, most first, so padding the folder
/// with filler can't push the real risk out of the lines shown.
fn finding_rank(what: &str) -> u8 {
    match what {
        "hooks" | "frontmatter-hooks" | "launch" | "apiKeyHelper" | "awsAuthRefresh" | "awsCredentialExport" | "otelHeadersHelper"
        | "statusLine" | "mcp-command" | "env" | "folderOpen-task" | "core.fsmonitor" | "core.hooksPath" | "hook"
        | "worktree-setup" | "environment" | "toolDiscoveryCommand" | "toolCallCommand" | "tools.discoveryCommand"
        | "tools.callCommand" => 0,
        "mcp-url" | "mcp-server" | "enableAllProjectMcpServers" | "enabledMcpjsonServers" | "enabledPlugins"
        | "extraKnownMarketplaces" | "filter" | "diff-driver" | "diff.external" | "credential.helper" | "include"
        | "core.sshCommand" | "core.pager" | "core.editor" | "core.askPass" | "gpg.program" | "sequence.editor"
        | "merge-driver" | "pager" | "script" => 1,
        "symlink" | "too-big" | "unreadable" | "unparseable" | "too-many-files" | "too-deep" | "not-fully-scanned" => 2,
        _ => 3,
    }
}

/// The card's words (PRD §3.4, reviews B#1, B#4): who asked, the folder,
/// how many findings, the most important ones in a code block, and a plain
/// note when a line was cut or more findings exist.
pub fn card_prompt(caller: &str, eval: &gate::Evaluation, actor: &Actor, leftover: &[String]) -> String {
    let tool = tool_display(actor.tool.as_deref());
    let caller = md_text(caller, 80);
    let folder = md_code_inline(&eval.path);
    let n = eval.findings.len();
    let mut s = if actor.changed_live {
        format!(
            "**The settings in {folder} changed while {caller} was running.** K2 stopped its sessions there until you decide. \
             {n} setting{} run commands or send your logins elsewhere when programs start here (Claude picks up new hooks without a restart):\n",
            if n == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "**{caller} wants to start {tool} in {folder}.** This folder has {n} setting{} that run commands or send your logins elsewhere when programs start here:\n",
            if n == 1 { "" } else { "s" }
        )
    };
    let mut ranked: Vec<&k2_core::cli_folder_trust::scan::Finding> = eval.findings.iter().collect();
    ranked.sort_by_key(|f| finding_rank(&f.what));
    let shown: Vec<String> = ranked.iter().take(CARD_LINES).map(|f| format!("{}: {}", f.file, f.detail)).collect();
    let truncated = shown.iter().any(|l| l.ends_with('…'));
    s.push_str(&md_code_block(&shown));
    s.push('\n');
    let status_cmd = md_code_inline(&format!("k2 workspace trust status {}", gate::shell_quote(&eval.path)));
    if n > CARD_LINES {
        s.push_str(&format!("{} more finding{} not shown: {status_cmd}\n", n - CARD_LINES, if n - CARD_LINES == 1 { "" } else { "s" }));
    }
    if truncated {
        s.push_str(&format!("Some lines are truncated; the full list: {status_cmd}\n"));
    }
    if let Some(why) = actor.why.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
        s.push_str(&format!("{caller} says: {}\n", md_text(why, 300)));
    }
    if !leftover.is_empty() {
        // Review round 2 M2: trust K2 didn't write (or can't tell from
        // yours) still covers this folder whatever you choose here.
        let shown: Vec<String> = leftover.iter().take(CARD_LINES).cloned().collect();
        s.push_str("Trust K2 didn't write still covers this folder in these tools, whatever you choose:\n");
        s.push_str(&md_code_block(&shown));
        s.push('\n');
        s.push_str(&format!("To remove it: {}\n", md_code_inline(&gate::forget_hint(&eval.path))));
    }
    s.push_str("If you didn't put these there, don't trust it.");
    s
}

/// Who asked, for the card: the actor's own caller, else the agent whose
/// passport made this request, else the folder's agent.
fn caller_name(actor: &Actor, folder_project: Option<&str>) -> String {
    if let Some(c) = actor.caller.as_deref().filter(|c| !c.trim().is_empty()) {
        return c.to_string();
    }
    if let Some(p) = crate::caller_workspace::request_principal().filter(|p| !p.workspace_uuid.is_empty()) {
        return agent_name_of(&p.workspace_uuid);
    }
    if let Some(person) = request_person().filter(|p| !p.trim().is_empty()) {
        return person;
    }
    let folder_agent = folder_project.map(agent_name_of).unwrap_or_else(|| "An agent".to_string());
    match actor.by.as_str() {
        "heartbeat" => format!("{folder_agent}'s heartbeat"),
        _ => folder_agent,
    }
}

fn ticket_title(eval: &gate::Evaluation) -> String {
    let fp = eval.fingerprint.as_deref().unwrap_or("");
    format!("Trust {}? (settings {})", eval.path, &fp[..fp.len().min(8)])
}

/// The presenter: a Thread card where there is a Thread, else an owner
/// ticket. Never fails a spawn; where nothing can be posted, the 409 body
/// still says how to decide.
fn present(ask: &Ask<'_>) -> Asked {
    let eval = ask.eval;
    let folder = project_for_path(&eval.path).or_else(|| project_for_path(&eval.scope)).map(|(id, _)| id);
    let principal = crate::caller_workspace::request_principal().map(|p| p.workspace_uuid).filter(|w| !w.is_empty());
    let mut candidates: Vec<String> = Vec::new();
    for c in [folder.clone(), ask.actor.project_id.clone(), principal] {
        if let Some(c) = c.filter(|c| !c.is_empty()) {
            if !candidates.contains(&c) {
                candidates.push(c);
            }
        }
    }
    let caller = caller_name(ask.actor, folder.as_deref());
    let leftover = k2_core::cli_folder_trust::current_home()
        .map(|h| k2_core::cli_folder_trust::leftover_grants(&h, &eval.keys, eval.in_git))
        .unwrap_or_default();
    let prompt = card_prompt(&caller, eval, ask.actor, &leftover);
    let action = k2_core::overlay::CardAction {
        id: CARD_ACTION.to_string(),
        params: json!({
            "path": eval.path,
            "scope": eval.scope,
            "fingerprint": eval.fingerprint,
            "caller": caller,
        }),
        dedupe: eval.scope.clone(),
        result: None,
        view: None,
    };
    let options = [TRUST_IT, ALWAYS, DONT];
    let title = ticket_title(eval);
    let ticket_prefix = format!("Trust {}? ", eval.path);
    // Tickets for this folder: the one for these settings stays; one for
    // older settings is resolved (review B#3: refresh on a new fingerprint).
    let open_ticket = |project: &str| -> Option<String> {
        let mut current = None;
        for i in k2_core::feedback::list_for_project(project, &k2_core::feedback::ListFilter::Open).unwrap_or_default() {
            if i.title == title {
                current = Some(i.id);
            } else if i.title.starts_with(&ticket_prefix) {
                if let Err(e) = k2_core::feedback::set_status(&i.id, "resolved") {
                    log_debug!("[folder-trust] resolve stale ticket {}: {e}", i.id);
                }
            }
        }
        current
    };
    for project in &candidates {
        let Some(conversation) = pinned_conversation(project) else { continue };
        match crate::overlay_routes::present_action_card(
            Some(&conversation),
            project,
            &prompt,
            &options,
            action.clone(),
            true,
            k2_core::overlay::now_secs() + k2_core::overlay::DEFAULT_CARD_EXPIRY_SECS,
            crate::overlay_routes::CardAudience::AgentPost,
        ) {
            Ok(crate::overlay_routes::Presented::Card { id, .. }) | Ok(crate::overlay_routes::Presented::Waiting { id }) => {
                log_debug!("[folder-trust] card {id} for {} on {project}", eval.scope);
                let ticket = candidates.first().and_then(|p| open_ticket(p));
                return Asked { card: Some(id), conversation: Some(conversation), project: Some(project.clone()), ticket };
            }
            Ok(crate::overlay_routes::Presented::NoThread) => {}
            Err(e) => log_debug!("[folder-trust] card for {} on {project}: {e}", eval.scope),
        }
    }
    // No person and no Thread (A6): an owner ticket with the findings, the
    // three choices and the fingerprint they apply to.
    let Some(project) = candidates.first() else {
        log_debug!("[folder-trust] {}: no workspace to post a card or ticket on", eval.scope);
        return Asked::default();
    };
    if let Some(id) = open_ticket(project) {
        return Asked { ticket: Some(id), project: Some(project.clone()), ..Default::default() };
    }
    let fp = eval.fingerprint.as_deref().unwrap_or("");
    // Commands in a code block, so a path can't add markdown (review round
    // 2 Low).
    let q = gate::shell_quote(&eval.path);
    let commands = md_code_block(&[
        "# Trust it (this version):".to_string(),
        format!("k2 workspace trust {q} --yes --fingerprint {fp}"),
        "# Always trust this folder:".to_string(),
        format!("k2 workspace trust {q} --always --fingerprint {fp}"),
        "# Don't trust:".to_string(),
        format!("k2 workspace trust {q} --no --fingerprint {fp}"),
    ]);
    let body = format!(
        "{prompt}\n\nThere is no Thread to put a card on, so K2 filed this ticket. Choose one (owner):\n\n{commands}\n\n\
         If these settings change, K2 refuses these commands and files a new ticket. \
         Until then K2 starts no program in this folder but a plain terminal. Settings → Workspaces → Folder trust shows it too."
    );
    match crate::feedback_routes::file_owners_only_notice(project, &agent_name_of(project), &title, &body) {
        Ok(item) => Asked { ticket: Some(item.id), project: Some(project.clone()), ..Default::default() },
        Err(e) => {
            log_debug!("[folder-trust] ticket for {}: {e}", eval.scope);
            Asked::default()
        }
    }
}

// ── Spawn gate, held spawns, heartbeats ──────────────────────────────

/// A spawn held for a scope: the request, when, and who asked (review B#8).
struct HeldSpawn {
    req: crate::spawn::SpawnWorkspaceSessionRequest,
    at: Instant,
    principal: Option<crate::session_token::HookPrincipal>,
}

/// Spawns held for a scope, retried once when the person trusts it.
static HELD: Mutex<Option<HashMap<String, Vec<HeldSpawn>>>> = Mutex::new(None);
/// At most this many held spawns per scope (the newest per map key wins).
const HELD_MAX: usize = 16;

/// The spawn door: `Err(refusal)` when the folder is held or denied.
/// Every program is gated but a plain interactive login shell with no env
/// of its own (reviews A-H2, round 2 H2). `env` is what the caller and the
/// agent's launch block set (not K2's own injections).
///
/// A workspace spawn whose cwd is somewhere else (a persona `launch:`
/// block's `cwd:`, review round 2 H1) is gated on the workspace folder
/// that holds the persona too: both must pass.
pub fn gate_spawn(
    program: Option<&str>,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &str,
    env_home: Option<&str>,
    project_id: Option<&str>,
    by: &str,
) -> Result<(), Refusal> {
    let actor = Actor {
        project_id: project_id.map(str::to_string),
        by: by.to_string(),
        tool: program.map(str::to_string),
        ..Default::default()
    };
    if k2_core::cli_folder_trust::needs_gate(program, args, env) {
        if let Some(ws) = project_id.and_then(project_path_for_id) {
            let same = match (dunce::canonicalize(&Path::new(&ws)), dunce::canonicalize(&Path::new(cwd))) {
                (Ok(a), Ok(b)) => a == b,
                _ => ws == cwd,
            };
            if !same {
                k2_core::cli_folder_trust::gate_harness_spawn(program, args, env, Some(Path::new(&ws)), env_home, &actor)?;
            }
        }
    }
    k2_core::cli_folder_trust::gate_harness_spawn(program, args, env, Some(Path::new(cwd)), env_home, &actor).map(|_| ())
}

/// Session-map keys of shells that were gated only because of their env
/// (a launch block's `PROMPT_COMMAND`, `ZDOTDIR`…): their program looks
/// like a plain shell, but "Don't trust" and the watcher must treat them
/// as gated (review round 3). Kept in memory; after a daemon restart an
/// adopted session is judged by its program and args only.
static ENV_GATED: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

/// Record how a spawn under `key` was judged (call after the gate passed).
pub fn note_spawn(key: &str, program: Option<&str>, args: &[String], env: &HashMap<String, String>) {
    let by_env = k2_core::cli_folder_trust::needs_gate(program, args, env)
        && !k2_core::cli_folder_trust::needs_gate(program, args, &HashMap::new());
    let mut g = ENV_GATED.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(Default::default);
    if by_env {
        set.insert(key.to_string());
    } else {
        set.remove(key);
    }
}

/// Whether a live session in the map is a gated one (not a person's plain
/// shell): by its program and args, or because its env made it gated.
pub fn session_is_gated(key: &str, program: Option<&str>, args: &[String]) -> bool {
    if k2_core::cli_folder_trust::needs_gate(program, args, &HashMap::new()) {
        return true;
    }
    let g = ENV_GATED.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().is_some_and(|s| s.contains(key))
}

/// The gate for reusing a live session under `key`: like [`gate_spawn`],
/// and a session recorded as gated by its env ([`note_spawn`]) stays
/// gated even when this request carries no env (review round 3).
#[allow(clippy::too_many_arguments)]
pub fn gate_live(
    key: &str,
    program: Option<&str>,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &str,
    env_home: Option<&str>,
    project_id: Option<&str>,
    by: &str,
) -> Result<(), Refusal> {
    let recorded = {
        let g = ENV_GATED.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().is_some_and(|s| s.contains(key))
    };
    if recorded && !k2_core::cli_folder_trust::needs_gate(program, args, env) {
        // A marker key outside K2's own (`K2_*`) keeps the shell gated.
        let mut env = env.clone();
        env.insert(RECORDED_ENV_MARKER.to_string(), "1".to_string());
        return gate_spawn(program, args, &env, cwd, env_home, project_id, by);
    }
    gate_spawn(program, args, env, cwd, env_home, project_id, by)
}

/// See [`gate_live`]; never set on a child.
const RECORDED_ENV_MARKER: &str = "K2FT_RECORDED_ENV";

/// Queue a held spawn (not heartbeat lanes: those are skipped), with the
/// request's caller, for at most [`HELD_TTL`].
pub fn hold_spawn(scope: &str, req: &crate::spawn::SpawnWorkspaceSessionRequest) {
    if req.canonical_key.as_deref().is_some_and(|k| k.contains(":hb:")) {
        return;
    }
    let key = crate::spawn::canonical_key_for_request(req);
    let mut g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    for list in map.values_mut() {
        list.retain(|h| h.at.elapsed() < HELD_TTL);
    }
    let list = map.entry(scope.to_string()).or_default();
    list.retain(|h| crate::spawn::canonical_key_for_request(&h.req) != key);
    list.push(HeldSpawn { req: req.clone(), at: Instant::now(), principal: crate::caller_workspace::request_principal() });
    if list.len() > HELD_MAX {
        list.remove(0);
    }
}

fn take_held(scope: &str) -> Vec<HeldSpawn> {
    let mut g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    g.as_mut().and_then(|m| m.remove(scope)).unwrap_or_default()
}

/// How many spawns are held for `scope` (tests, status).
pub fn held_count(scope: &str) -> usize {
    let g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(scope)).map(Vec::len).unwrap_or(0)
}

#[cfg(test)]
fn age_held_for_tests(scope: &str, by: Duration) {
    let mut g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(list) = g.as_mut().and_then(|m| m.get_mut(scope)) {
        for h in list {
            h.at = h.at.checked_sub(by).unwrap_or(h.at);
        }
    }
}

/// Heartbeats: `Some(refusal)` when the workspace's folder is held or
/// denied, so the fire is skipped (an activity row says `held: folder
/// trust`). A held folder still gets its card.
pub fn heartbeat_hold(project_path: &str, project_id: &str) -> Option<Refusal> {
    let program = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_resolve::resolve_agent_command(&conn, project_path).command
    };
    heartbeat_hold_for(project_path, project_id, &program)
}

/// [`heartbeat_hold`] with the agent's program given. Gated like a spawn:
/// every program but a plain shell (an empty program is no agent).
pub fn heartbeat_hold_for(project_path: &str, project_id: &str, program: &str) -> Option<Refusal> {
    if !k2_core::cli_folder_trust::needs_gate(Some(program), &[], &HashMap::new()) {
        return None;
    }
    let actor = Actor {
        project_id: Some(project_id.to_string()),
        by: "heartbeat".into(),
        tool: Some(program.to_string()),
        ..Default::default()
    };
    match gate::ensure(project_path, Door::Spawn, &actor, None) {
        Outcome::Held(r) | Outcome::Denied(r) => Some(r),
        _ => None,
    }
}

/// After a person's decision: retry held spawns once on trust (each as the
/// caller that asked, if under [`HELD_TTL`]); on "Don't trust", drop them
/// and stop K2's running AI sessions in that scope. Returns how many
/// sessions were stopped.
pub fn after_decision(scope: &str, decision: Decision, granted: bool) -> usize {
    match decision {
        Decision::Once | Decision::Always if granted => {
            let held: Vec<HeldSpawn> = take_held(scope).into_iter().filter(|h| h.at.elapsed() < HELD_TTL).collect();
            if !held.is_empty() {
                // The spawn registers the session and emits session events,
                // which need the daemon's tokio runtime: carry its handle
                // into the retry thread (a request worker has one).
                let rt = tokio::runtime::Handle::try_current().ok();
                std::thread::Builder::new()
                    .name("k2-folder-trust-retry".into())
                    .spawn(move || {
                        let _rt = rt.as_ref().map(|h| h.enter());
                        for h in held {
                            let key = crate::spawn::canonical_key_for_request(&h.req);
                            let r = crate::caller_workspace::with_request_principal(h.principal, || {
                                crate::spawn::spawn_agent_session_v2_blocking(h.req)
                            });
                            match r {
                                Ok(o) => log_debug!("[folder-trust] held spawn {key} started (session {})", o.session_id),
                                Err(e) => log_debug!("[folder-trust] held spawn {key}: {e}"),
                            }
                        }
                    })
                    .ok();
            }
            0
        }
        Decision::Deny => {
            take_held(scope);
            stop_sessions_in(scope)
        }
        _ => 0,
    }
}

/// Stop K2's live AI sessions whose own folder's scope is `scope` (review
/// B#5: never a nested workspace with its own scope). A plain shell is a
/// person's terminal and stays.
pub fn stop_sessions_in(scope: &str) -> usize {
    let mut n = 0;
    for (key, s) in crate::v2_session_map::snapshot() {
        if !session_is_gated(&key, s.program.as_deref(), &s.args) {
            continue;
        }
        let Some(cwd) = s.cwd.as_ref() else { continue };
        if gate::scope_for(&cwd.to_string_lossy()).as_deref() != Some(scope) {
            continue;
        }
        if crate::v2_session_map::unregister(&key).is_some() {
            log_debug!("[folder-trust] stopped {key} ({}): folder not trusted", cwd.display());
            n += 1;
        }
    }
    n
}

// ── 409 (spawn routes only, review B#6) ──────────────────────────────

/// The 409 body for a spawn error that carries the gate's token.
pub fn refusal_response(e: &str) -> Option<CliResponse> {
    gate::refusal_in_text(e).map(|r| CliResponse { status: "409 Conflict", content_type: "application/json", body: r.to_json().to_string() })
}

/// A spawn error for a spawn route: the refusal's 409, else the old 400.
pub fn spawn_failed_response(e: &str) -> CliResponse {
    refusal_response(e).unwrap_or_else(|| CliResponse::bad_request(format!("spawn failed: {e}")))
}

// ── The card's answer (action_cards::run) ────────────────────────────

/// A person tapped an option on a `folder.trust` card (already checked to
/// be the owner by `require_action_card_person`). `by`: `owner` or a login.
/// The tap counts only while the folder still resolves to the card's path
/// and scope with the card's fingerprint (review B#2), for every answer;
/// otherwise nothing is recorded and a fresh card is posted.
pub fn run_card(params: &J, answer: &str, by: &str) -> Result<J, String> {
    let path = params["path"].as_str().filter(|p| !p.is_empty()).ok_or("the card names no folder")?;
    let decision = match answer {
        TRUST_IT => Decision::Once,
        ALWAYS => Decision::Always,
        DONT => Decision::Deny,
        other => return Err(format!("'{other}' isn't an answer to this card")),
    };
    let seen = Seen { path: Some(path), scope: params["scope"].as_str(), fingerprint: params["fingerprint"].as_str() };
    let pending = gate::scope_for(path).as_deref().and_then(gate::pending_of);
    let r = match gate::decide_checked(path, decision, by, seen) {
        Ok(r) => r,
        Err(DecideError::Changed(why)) => {
            let fresh = gate::request(path, &Actor { by: "recheck".into(), caller: Some("K2".into()), ..Default::default() });
            let next = match fresh {
                Outcome::Held(r) if r.card.is_some() => "K2 posted a new card for what's there now.".to_string(),
                Outcome::Held(_) => "Decide on what's there now: k2 workspace trust status shows it.".to_string(),
                Outcome::Granted { .. } => "There's nothing risky to decide now.".to_string(),
                Outcome::Denied(_) => "The folder is already not trusted.".to_string(),
                Outcome::Skipped(why) => format!("K2 can't check it now ({why})."),
            };
            return Err(format!("Nothing was decided: {why}. {next}"));
        }
        Err(DecideError::Failed(e)) => return Err(e),
    };
    if let Some(p) = pending {
        close_pending(&p, false);
    }
    let stopped = after_decision(&r.scope, decision, r.granted);
    let mut message = r.message.clone();
    if stopped > 0 {
        message.push_str(&format!(" Stopped {stopped} running session{} there.", if stopped == 1 { "" } else { "s" }));
    }
    Ok(json!({
        "decision": decision.as_str(),
        "path": r.path,
        "granted": r.granted,
        "leftover": r.leftover,
        "stopped": stopped,
        "message": message,
    }))
}

/// After a decision: resolve the owner ticket and (when the decision came
/// from elsewhere than the card) void the waiting card.
fn close_pending(p: &gate::Pending, void_card: bool) {
    if void_card {
        if let (Some(conv), Some(card), Some(project)) = (p.conversation.as_deref(), p.card.as_deref(), p.project.as_deref()) {
            match k2_core::overlay::void_card(conv, project, card, None) {
                Ok(cb) => crate::overlay_routes::publish_action_frame(&cb),
                Err(e) => log_debug!("[folder-trust] void card {card}: {e}"),
            }
        }
    }
    if let Some(t) = p.ticket.as_deref() {
        if let Err(e) = k2_core::feedback::set_status(t, "resolved") {
            log_debug!("[folder-trust] resolve ticket {t}: {e}");
        }
    }
}

// ── Routes ───────────────────────────────────────────────────────────

fn param(params: &HashMap<String, String>, body: &[u8], key: &str) -> Option<String> {
    if let Some(v) = params.get(key).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Some(v);
    }
    let v: J = serde_json::from_slice(body).ok()?;
    v.get(key).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn err(status: &'static str, code: &str, message: impl std::fmt::Display) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: json!({ "error": code, "message": message.to_string() }).to_string() }
}

fn folder_param(params: &HashMap<String, String>, body: &[u8]) -> Result<String, CliResponse> {
    param(params, body, "path")
        .or_else(|| param(params, body, "folder"))
        .ok_or_else(|| err("400 Bad Request", "usage", "path required (the folder)"))
}

/// The passport's workspace, when an agent asks (`None` for a person).
fn passport_workspace(params: &HashMap<String, String>) -> Option<String> {
    let bound = params.get(crate::caller_workspace::PRINCIPAL_BOUND_KEY).map(String::as_str) == Some("1")
        || params.contains_key("cell_session_id");
    crate::caller_workspace::principal_from_params(params)
        .map(|p| p.workspace_uuid)
        .or_else(|| crate::caller_workspace::request_principal().map(|p| p.workspace_uuid))
        .filter(|w| !w.is_empty())
        .or_else(|| bound.then(String::new))
}

/// Which registered workspace folder `folder` is, for this caller (reviews
/// A-H1, B#7): a registered workspace folder only, and for an agent only
/// its own workspace's folder. `Err` is the refusal.
fn workspace_folder_for(params: &HashMap<String, String>, folder: &str) -> Result<(String, String), CliResponse> {
    let Some((id, path)) = registered_exact(folder) else {
        return Err(err(
            "404 Not Found",
            "not_a_workspace",
            format!("{folder} isn't a registered workspace folder; folder trust only covers workspaces"),
        ));
    };
    if let Some(own) = passport_workspace(params) {
        if own != id {
            return Err(err("403 Forbidden", "not_your_workspace", "an agent can only ask about or read its own workspace's folder"));
        }
    }
    Ok((id, path))
}

/// `/cli/folder-trust/*`. `session_author`: `owner`, a login name, or
/// `agent` (the dispatcher's), used by the person check on decide/reset
/// and to name the caller on the card.
pub fn dispatch(is_post: bool, path: &str, params: &HashMap<String, String>, body: &[u8], session_author: &str) -> CliResponse {
    match path {
        STATUS_PATH => status_route(params, body),
        REQUEST_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            request_route(params, body, session_author)
        }
        DECIDE_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            if let Err(r) = crate::overlay_routes::require_action_card_person(params, session_author, "answers") {
                return r;
            }
            decide_route(params, body, session_author)
        }
        RESET_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            if let Err(r) = crate::overlay_routes::require_action_card_person(params, session_author, "answers") {
                return r;
            }
            reset_route(params, body, session_author)
        }
        FORGET_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            if let Err(r) = crate::overlay_routes::require_action_card_person(params, session_author, "answers") {
                return r;
            }
            forget_route(params, body, session_author)
        }
        _ => CliResponse::not_found(),
    }
}

fn status_json(id: &str, s: &gate::StatusView) -> J {
    let mut v = serde_json::to_value(s).unwrap_or(J::Null);
    v["projectId"] = json!(id);
    v
}

fn status_route(params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    match param(params, body, "path").or_else(|| param(params, body, "folder")) {
        Some(p) => {
            let (id, path) = match workspace_folder_for(params, &p) {
                Ok(w) => w,
                Err(r) => return r,
            };
            match gate::status(&path) {
                Ok(v) => CliResponse::ok_json(status_json(&id, &v).to_string()),
                Err(e) => err("400 Bad Request", "usage", e),
            }
        }
        None => {
            // Every registered workspace (Settings → Workspaces); an agent
            // sees only its own.
            let projects = match k2_core::projects_ops::projects_list() {
                Ok(p) => p,
                Err(e) => return CliResponse::internal_error(e),
            };
            let own = passport_workspace(params);
            let folders: Vec<J> = projects
                .iter()
                .filter(|p| own.as_deref().is_none_or(|o| o == p.id))
                .filter_map(|p| gate::status(&p.path).ok().map(|s| status_json(&p.id, &s)))
                .collect();
            CliResponse::ok_json(json!({ "folders": folders }).to_string())
        }
    }
}

fn request_route(params: &HashMap<String, String>, body: &[u8], session_author: &str) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let (id, path) = match workspace_folder_for(params, &folder) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let caller = match passport_workspace(params) {
        Some(ws) if !ws.is_empty() => agent_name_of(&ws),
        Some(_) => "An agent".to_string(),
        None if session_author == "owner" => "The owner".to_string(),
        None => session_author.to_string(),
    };
    let actor = Actor {
        project_id: Some(id),
        by: "request".into(),
        why: param(params, body, "why"),
        caller: Some(caller),
        ..Default::default()
    };
    // Only asks: no grant is written or removed (review A-H1).
    let out = gate::request(&path, &actor);
    CliResponse::ok_json(out.to_json().to_string())
}

fn decision_param(params: &HashMap<String, String>, body: &[u8]) -> Result<Decision, CliResponse> {
    let raw = param(params, body, "decision").unwrap_or_default();
    Decision::parse(&raw).ok_or_else(|| err("400 Bad Request", "usage", "decision must be once (--yes), always (--always) or deny (--no)"))
}

fn decide_route(params: &HashMap<String, String>, body: &[u8], by: &str) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let decision = match decision_param(params, body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // The fingerprint the owner saw (status, or the ticket) is required
    // (review B#3): "none" means they saw a folder with no risky settings.
    let Some(fp) = param(params, body, "fingerprint") else {
        return err(
            "400 Bad Request",
            "usage",
            "fingerprint required: the CLI sends the one `k2 workspace trust status` showed (or the ticket's)",
        );
    };
    let fp = (fp != "none").then_some(fp);
    let scope_seen = param(params, body, "scope");
    let scope = gate::scope_for(&folder);
    let pending = scope.as_deref().and_then(gate::pending_of);
    let seen = Seen { path: None, scope: scope_seen.as_deref(), fingerprint: fp.as_deref() };
    let r = match gate::decide_checked(&folder, decision, by, seen) {
        Ok(r) => r,
        Err(DecideError::Changed(why)) => {
            // Refresh the ask (a new card or ticket for what's there now).
            let _ = gate::request(&folder, &Actor { by: "recheck".into(), caller: Some("K2".into()), ..Default::default() });
            let current = gate::status(&folder).ok().map(|s| serde_json::to_value(&s).unwrap_or(J::Null));
            return CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body: json!({
                    "error": CHANGED,
                    "message": format!("Nothing was decided: {why}. Look again: k2 workspace trust status {}", gate::shell_quote(&folder)),
                    "current": current,
                })
                .to_string(),
            };
        }
        Err(DecideError::Failed(e)) => return err("400 Bad Request", "usage", e),
    };
    // The card that was waiting no longer asks anything.
    if let Some(p) = pending {
        close_pending(&p, true);
    }
    let stopped = after_decision(&r.scope, decision, r.granted);
    let mut v = serde_json::to_value(&r).unwrap_or(J::Null);
    v["ok"] = json!(true);
    v["stopped"] = json!(stopped);
    CliResponse::ok_json(v.to_string())
}

fn reset_route(params: &HashMap<String, String>, body: &[u8], by: &str) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    match gate::reset(&folder, by) {
        Ok(st) => {
            take_held(&st.scope);
            let mut v = serde_json::to_value(&st).unwrap_or(J::Null);
            v["ok"] = json!(true);
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => err("400 Bad Request", "usage", e),
    }
}

/// `POST /cli/folder-trust/forget` (owner, review round 2 M2): remove
/// every trust entry for the folder in every CLI's store, the person's own
/// and parent folders' included. 200 with what was removed; a store K2
/// couldn't change is a 500 that names it.
fn forget_route(params: &HashMap<String, String>, body: &[u8], by: &str) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    match gate::forget(&folder, by) {
        Ok(f) if f.errors.is_empty() => {
            let mut v = serde_json::to_value(&f).unwrap_or(J::Null);
            v["ok"] = json!(true);
            CliResponse::ok_json(v.to_string())
        }
        Ok(f) => {
            let mut v = serde_json::to_value(&f).unwrap_or(J::Null);
            v["ok"] = json!(false);
            v["error"] = json!("forget_incomplete");
            v["message"] = json!(format!("K2 couldn't change: {}", f.errors.join("; ")));
            CliResponse { status: "500 Internal Server Error", content_type: "application/json", body: v.to_string() }
        }
        Err(e) => err("400 Bad Request", "usage", e),
    }
}

// ── Boot ─────────────────────────────────────────────────────────────

/// After ready: finish the 0.46.3 upgrade (one owner summary listing the
/// folders that were trusted before K2 checked and are risky now), then
/// re-assert decisions that grant (the sweep never asks in bulk).
pub fn boot_pass() {
    let upgraded = gate::upgrade_pass();
    if !upgraded.is_empty() {
        file_upgrade_summary(&upgraded);
    }
    k2_core::cli_folder_trust::trust_all_registered_projects();
}

fn file_upgrade_summary(folders: &[gate::UpgradedFolder]) {
    let Some((project, _)) = folders.iter().find_map(|f| project_for_path(&f.path)) else {
        log_debug!("[folder-trust] upgrade summary: no workspace to file it on");
        return;
    };
    let title = "Folders trusted before K2 checked them".to_string();
    let mut body = String::from(
        "K2 now looks inside a folder before it tells Claude, Codex, Grok, Gemini or Cursor to trust it. \
         These folders were trusted before K2 checked, and they have settings that run commands when the AI tools start. \
         They stay trusted as they are; K2 asks again if these settings change.\n",
    );
    for f in folders {
        body.push_str(&format!("\n{}\n", md_text(&f.path, 300)));
        let lines: Vec<String> = f.findings.iter().take(CARD_LINES).map(|l| format!("{}: {}", l.file, l.detail)).collect();
        body.push_str(&md_code_block(&lines));
        body.push('\n');
        let status_cmd = md_code_inline(&format!("k2 workspace trust status {}", gate::shell_quote(&f.path)));
        if f.findings.len() > CARD_LINES || f.more > 0 {
            body.push_str(&format!("More: {status_cmd}\n"));
        }
        body.push_str(&format!("Review: {status_cmd}, then --yes / --always / --no\n"));
        // Review round 2 M2: entries K2 can't tell from the person's (Codex,
        // Grok, Claude entries with more than the trust flag) stay whatever
        // the decision; say so, with the command that removes them.
        let leftover = k2_core::cli_folder_trust::current_home()
            .map(|h| {
                let keys = vec![f.path.clone()];
                k2_core::cli_folder_trust::leftover_grants(&h, &keys, true)
            })
            .unwrap_or_default();
        if !leftover.is_empty() {
            let shown: Vec<String> = leftover.iter().take(CARD_LINES).cloned().collect();
            body.push_str("Trust K2 didn't write (or can't tell from yours) stays whatever you decide:\n");
            body.push_str(&md_code_block(&shown));
            body.push_str(&format!("\nTo remove it: {}\n", md_code_inline(&gate::forget_hint(&f.path))));
        }
    }
    body.push_str("\nSettings → Workspaces → Folder trust shows each one, with Reset.");
    let agent = agent_name_of(&project);
    if let Err(e) = crate::feedback_routes::file_owners_only_notice(&project, &agent, &title, &body) {
        log_debug!("[folder-trust] upgrade summary: {e}");
    }
}

#[cfg(test)]
#[path = "folder_trust_tests.rs"]
mod tests;
