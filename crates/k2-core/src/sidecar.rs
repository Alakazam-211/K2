//! `k2 sidecar` v1 — the daemon-independent pieces
//! (`.k2/prds/prd-k2-sidecar-cli-v1.md`, vs-live SC28–SC48).
//!
//! A sidecar is an ordinary `tab-<pane group>` harness session (SC3).
//! This module holds what the daemon routes need and what can be tested
//! without a PTY:
//!
//! - name rules (§6.1, SC45) and the per-workspace cap (SC14);
//! - the harness table for the Big 7 and how each one gets a durable
//!   conversation id ([`IdentityStyle`]);
//! - [`resume_argv`], the ONE helper restart recovery, the `k2 msg` wake
//!   and `new`-resume build resume argv with (SC48);
//! - the brief file (§6.4) and the pointer line;
//! - tab-row sidecar columns (migration 0132), the read-only list (SC43);
//! - the daemon-written layout tab (SC34);
//! - conversation discovery for harnesses that mint their own ids
//!   (codex, hermes) and the adoption that binds the id to the tab row.
//!
//! ## Per-harness identity (Rosson 2026-10-07: Big 7 in v1)
//!
//! | harness | new sidecar | stored conversation id | resume |
//! |---|---|---|---|
//! | claude | `--session-id <uuid>` | uuid | `--resume <uuid>` when the transcript exists, else `--session-id <uuid>` again |
//! | grok   | `--session-id <uuid>` | uuid | same as claude |
//! | gemini | `--session-id <uuid>` | uuid | same as claude (gemini refuses `--session-id` on an existing id and `--resume` on a missing one) |
//! | pi     | `--session <file>` (a new file under `~/.pi/agent/sessions/<cwd>/`) | the file path | `--session <file>` (pi loads it, or starts fresh at that path) |
//! | cursor | `cursor-agent create-chat` → id, then `--resume <id>` | chat id | `--resume <id>` |
//! | codex  | bare; id discovered from `~/.codex/sessions` after start | discovered uuid | `codex resume <uuid>`; before discovery: a fresh codex that re-reads BRIEF.md |
//! | hermes | bare; id discovered from `~/.hermes/state.db` | discovered id | `hermes --resume <id>`; before discovery: a fresh hermes that re-reads BRIEF.md |

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::workspace::provider_resume::{
    provider_resume_for_command, ProviderResume, ResumeGrammar,
};
use crate::workspace_session_handles as handles;

/// Live sidecars allowed per workspace (SC14). `K2_SIDECAR_CAP` overrides.
pub const DEFAULT_CAP: usize = 8;
/// Largest brief, in bytes (SC9).
pub const BRIEF_MAX_BYTES: usize = 64 * 1024;
/// Request body cap for `new` (SC9).
pub const NEW_BODY_MAX_BYTES: usize = 96 * 1024;
/// Request body cap for `stop` and `agent-access/set` (§5.1).
pub const SMALL_BODY_MAX_BYTES: usize = 4 * 1024;
/// Longest display name (§6.1).
pub const NAME_MAX_CHARS: usize = 40;

/// The Big 7 providers a v1 sidecar may run.
pub const SUPPORTED_PROVIDERS: &[&str] = &[
    "claude", "grok", "codex", "gemini", "cursor", "pi", "hermes",
];

/// Effective live cap.
pub fn cap() -> usize {
    std::env::var("K2_SIDECAR_CAP")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_CAP)
}

/// A refusal with its wire code, HTTP status and a hint (§5.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarError {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
}

impl SidecarError {
    pub fn new(code: &'static str, status: u16, hint: impl Into<String>) -> Self {
        Self {
            code,
            status,
            hint: hint.into(),
        }
    }
}

// ── Names (§6.1, SC45) ──────────────────────────────────────────────

/// Suggest a clean address token for a refused name (`Gardens & Apps`
/// → `gardens-and-apps`).
pub fn suggest_slug(name: &str) -> String {
    let lowered = name.trim().to_lowercase().replace('&', " and ");
    let mut out = String::new();
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.chars()
        .take(NAME_MAX_CHARS)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Validate a CLI sidecar name and return its slug (the handle).
/// Stricter than a Chats rename: the slug must be a clean address token,
/// not all digits, at most 40 characters, and not the workspace handle.
pub fn validate_name(name: &str, workspace_handle: &str) -> Result<String, SidecarError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(SidecarError::new(
            "bad_name",
            400,
            "give the sidecar a name",
        ));
    }
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(SidecarError::new(
            "bad_name",
            400,
            format!("names are at most {NAME_MAX_CHARS} characters"),
        ));
    }
    let refused = |why: &str| {
        let suggestion = suggest_slug(trimmed);
        let hint = if suggestion.is_empty() || suggestion.chars().all(|c| c.is_ascii_digit()) {
            format!("{why}; use lowercase letters, digits and single hyphens")
        } else {
            format!("{why}; try '{suggestion}'")
        };
        SidecarError::new("bad_name", 400, hint)
    };
    let slug = handles::slugify_custom_name(trimmed).map_err(|e| refused(&e))?;
    if !handles::is_address_token(&slug) {
        return Err(refused(&format!(
            "'{trimmed}' is not a clean address (lowercase letters, digits, single hyphens)"
        )));
    }
    if slug.chars().all(|c| c.is_ascii_digit()) {
        return Err(SidecarError::new(
            "bad_name",
            400,
            "a name made only of digits would hide a numbered sidecar; add a letter",
        ));
    }
    if handles::address_tokens_match(&slug, workspace_handle) {
        return Err(SidecarError::new(
            "bad_name",
            400,
            format!("'{slug}' is the workspace's own address; pick another name"),
        ));
    }
    Ok(slug)
}

// ── Harnesses ────────────────────────────────────────────────────────

/// How a harness gets the conversation id a sidecar resumes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityStyle {
    /// K2 mints a uuid and passes `<flag> <uuid>` (claude, grok, gemini).
    Premint(&'static str),
    /// pi: `--session <file>` names a session file. A missing file starts
    /// a new conversation at that path; an existing one is loaded. The
    /// path is the stored conversation id.
    SessionFile,
    /// cursor-agent: `cursor-agent create-chat` prints a new empty chat
    /// id; `--resume <id>` opens it, the first time and every time after.
    CreateChat,
    /// codex, hermes: no way to choose the id. Spawn bare and discover the
    /// id the harness wrote ([`discover_conversation`]).
    Discover,
}

/// Identity style for a provider key (`provider_resume` provider string).
pub fn identity_style(provider: &str) -> Option<IdentityStyle> {
    match provider {
        "claude" | "grok" | "gemini" => Some(IdentityStyle::Premint("--session-id")),
        "pi" => Some(IdentityStyle::SessionFile),
        "cursor" => Some(IdentityStyle::CreateChat),
        "codex" | "hermes" => Some(IdentityStyle::Discover),
        _ => None,
    }
}

/// Provider for a spawn command, limited to the Big 7.
pub fn supported_provider(command: &str) -> Option<&'static ProviderResume> {
    provider_resume_for_command(command).filter(|p| SUPPORTED_PROVIDERS.contains(&p.provider))
}

/// A resolved preset (§5.2 step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPreset {
    pub preset_id: String,
    pub label: String,
    /// First token of the preset command (`claude`, `cursor-agent`).
    pub program: String,
    pub args: Vec<String>,
    pub provider: &'static str,
    /// Preset env (migration 0070). Values are never logged.
    pub env: HashMap<String, String>,
    /// The preset's declared skip-approval flags.
    pub danger_flags: Vec<String>,
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// Resolve `harness` like `k2 preset show`: preset id, then label (any
/// case), then the command's first token. Disabled presets never match.
pub fn resolve_preset(conn: &Connection, harness: &str) -> Result<HarnessPreset, SidecarError> {
    let wanted = harness.trim();
    if wanted.is_empty() {
        return Err(SidecarError::new(
            "harness_unknown",
            400,
            "pass --harness <preset> (see k2 preset list)",
        ));
    }
    let presets = crate::db::schema::AgentPreset::list(conn)
        .map_err(|e| SidecarError::new("harness_unknown", 400, format!("preset list: {e}")))?;
    let enabled: Vec<_> = presets.into_iter().filter(|p| p.enabled == 1).collect();
    let first_token =
        |cmd: &str| -> String { crate::workspace::agent_resolve::parse_command_string(cmd).0 };
    let hit = enabled
        .iter()
        .find(|p| p.id == wanted)
        .or_else(|| {
            enabled
                .iter()
                .find(|p| p.label.eq_ignore_ascii_case(wanted))
        })
        .or_else(|| {
            enabled.iter().find(|p| {
                let tok = first_token(&p.command);
                tok == wanted || basename(&tok) == wanted
            })
        })
        .or_else(|| {
            // `cursor` names the Cursor Agent preset too.
            enabled.iter().find(|p| {
                provider_resume_for_command(&first_token(&p.command))
                    .is_some_and(|a| a.provider == wanted)
            })
        })
        .ok_or_else(|| {
            SidecarError::new(
                "harness_unknown",
                400,
                format!("no enabled preset matches '{wanted}' (see k2 preset list)"),
            )
        })?;
    let (program, args) = crate::workspace::agent_resolve::parse_command_string(&hit.command);
    let provider = supported_provider(&program).map(|a| a.provider).ok_or_else(|| {
        SidecarError::new(
            "harness_not_supported",
            400,
            format!(
                "preset '{}' runs '{}'; sidecars run Claude, Grok, Codex, Gemini, Cursor Agent, Pi or Hermes",
                hit.label,
                basename(&program)
            ),
        )
    })?;
    let env = hit
        .env
        .as_deref()
        .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(raw).ok())
        .unwrap_or_default();
    let danger_flags = hit
        .danger_flags
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default();
    Ok(HarnessPreset {
        preset_id: hit.id.clone(),
        label: hit.label.clone(),
        program,
        args,
        provider,
        env,
        danger_flags,
    })
}

/// Flags that skip a harness's approvals (§7.3 rule 5), beyond a preset's
/// own `danger_flags`.
pub const SKIP_APPROVAL_FLAGS: &[&str] = &[
    "--dangerously-skip-permissions",
    "--always-approve",
    "--yolo",
    "--dangerously-bypass-approvals-and-sandbox",
    "--allow-all",
    "--force",
    "-f",
    "-y",
];

/// Does `args` carry a skip-approvals flag (the fixed list plus `extra`)?
pub fn carries_skip_approvals(args: &[String], extra: &[String]) -> bool {
    args.iter().any(|a| {
        SKIP_APPROVAL_FLAGS.contains(&a.as_str())
            || extra.iter().any(|f| f == a)
            || a == "--approval-mode=yolo"
    }) || args
        .windows(2)
        .any(|w| w[0] == "--approval-mode" && w[1] == "yolo")
}

/// Where the binary resolves for a spawn: the test shim dir when set,
/// else the enriched spawn PATH. `None` = not installed.
pub fn resolve_binary(program: &str) -> Option<PathBuf> {
    let guard = crate::terminal::agent_spawn_guard::GuardEnv::from_process();
    let search =
        crate::terminal::login_path::augmented_path(&crate::terminal::login_path::process_path());
    if guard.shim_dirs.is_some() {
        return crate::terminal::agent_spawn_guard::resolve_program(program, &search, &guard)
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_file());
    }
    let raw = Path::new(program);
    if raw.is_absolute() {
        return raw.is_file().then(|| raw.to_path_buf());
    }
    std::env::split_paths(&search)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(program))
        .find(|c| c.is_file())
}

/// Install hint for a missing harness binary.
pub fn install_hint(program: &str) -> String {
    match crate::terminal::ensure_cli::fixed_install_command(basename(program)) {
        Some(cmd) => format!(
            "'{}' is not installed on this computer; install it with: {cmd}",
            basename(program)
        ),
        None => format!(
            "'{}' is not installed on this computer (not found on the spawn PATH)",
            basename(program)
        ),
    }
}

// ── Conversation ids and resume argv ─────────────────────────────────

/// pi's per-cwd session folder (`~/.pi/agent/sessions/--<cwd>--/`), the
/// same encoding pi's `getDefaultSessionDir` uses.
pub fn pi_sessions_dir(project_path: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let trimmed = project_path.trim_start_matches(['/', '\\']);
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' {
                '-'
            } else {
                c
            }
        })
        .collect();
    Some(
        home.join(".pi")
            .join("agent")
            .join("sessions")
            .join(format!("--{safe}--")),
    )
}

/// A new pi session file path for a sidecar (pi's own file-name shape:
/// `<iso time>_<uuid>.jsonl`).
pub fn new_pi_session_path(project_path: &str) -> Option<PathBuf> {
    let dir = pi_sessions_dir(project_path)?;
    let stamp = chrono::Utc::now()
        .format("%Y-%m-%dT%H-%M-%S-%3fZ")
        .to_string();
    Some(dir.join(format!("{stamp}_{}.jsonl", uuid::Uuid::new_v4())))
}

/// Run `cursor-agent create-chat` in `cwd` and return the new chat id.
/// The program resolves through the test shim guard. 20 s cap.
pub fn cursor_create_chat(
    program: &str,
    cwd: &str,
    env: &HashMap<String, String>,
) -> Result<String, String> {
    let bin = resolve_binary(program).ok_or_else(|| install_hint(program))?;
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("create-chat")
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    cmd.env(
        "PATH",
        crate::terminal::login_path::augmented_path(&crate::terminal::login_path::process_path()),
    );
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cursor-agent create-chat: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("cursor-agent create-chat timed out".to_string());
            }
            Err(e) => return Err(format!("cursor-agent create-chat: {e}")),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("cursor-agent create-chat: {e}"))?;
    if !out.status.success() {
        return Err(format!("cursor-agent create-chat exited {}", out.status));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let id = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("")
        .to_string();
    let ok = (8..=128).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err("cursor-agent create-chat printed no chat id".to_string());
    }
    Ok(id)
}

/// Fresh identity for a NEW sidecar conversation: argv with the identity
/// flags appended, and the conversation id when the harness lets K2 know
/// it up front (`None` for codex / hermes — discovered later).
pub fn fresh_identity_args(
    provider: &str,
    program: &str,
    base_args: &[String],
    project_path: &str,
    env: &HashMap<String, String>,
) -> Result<(Vec<String>, Option<String>), String> {
    let mut args = strip_session_identity(program, base_args);
    match identity_style(provider) {
        Some(IdentityStyle::Premint(flag)) => {
            let cid = uuid::Uuid::new_v4().to_string();
            args.push(flag.to_string());
            args.push(cid.clone());
            Ok((args, Some(cid)))
        }
        Some(IdentityStyle::SessionFile) => {
            let path = new_pi_session_path(project_path)
                .ok_or_else(|| "no home directory for pi sessions".to_string())?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            let cid = path.to_string_lossy().into_owned();
            args.push("--session".to_string());
            args.push(cid.clone());
            Ok((args, Some(cid)))
        }
        Some(IdentityStyle::CreateChat) => {
            let cid = cursor_create_chat(program, project_path, env)?;
            args.push("--resume".to_string());
            args.push(cid.clone());
            Ok((args, Some(cid)))
        }
        Some(IdentityStyle::Discover) => Ok((args, None)),
        None => Err(format!("'{program}' is not a sidecar harness")),
    }
}

/// Drop every session-identity token from `args` in the command's own
/// grammar: `--session-id <v>`, `--resume <v>` / `-r <v>`, pi's
/// `--session <v>`, codex's `resume <v>` subcommand. Unknown commands
/// come back unchanged.
pub fn strip_session_identity(command: &str, args: &[String]) -> Vec<String> {
    let Some(adapter) = provider_resume_for_command(command) else {
        return args.to_vec();
    };
    let mut flags: Vec<&str> = vec!["--resume", "-r"];
    if let Some(f) = adapter.premint_flag() {
        flags.push(f);
    }
    if let ResumeGrammar::Flag(f) = adapter.grammar {
        flags.push(f);
    }
    if adapter.provider == "gemini" || adapter.provider == "claude" || adapter.provider == "grok" {
        flags.push("--session-id");
    }
    let sub = match adapter.grammar {
        ResumeGrammar::Subcommand(s) => Some(s),
        ResumeGrammar::Flag(_) => None,
    };
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let takes_value = flags.contains(&a) || sub == Some(a);
        if takes_value && i + 1 < args.len() && !args[i + 1].starts_with('-') {
            i += 2;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// Does the harness already hold a transcript for `sid`? Only asked for
/// the premint harnesses, whose `--resume` fails on a missing id and whose
/// `--session-id` fails on an existing one.
pub fn conversation_exists(provider: &str, sid: &str, project_path: &str) -> bool {
    match provider {
        "gemini" => crate::chat_history::gemini_session_id_exists(sid),
        "pi" => Path::new(sid).is_file(),
        _ => crate::workspace::provider_resume::provider_resume_for_provider(provider)
            .is_some_and(|a| a.session_file_exists(sid, project_path)),
    }
}

/// SC48 — THE resume argv for a stored conversation `sid`, shared by
/// restart recovery (`v2_spawn::recovered_launch`), the `k2 msg` sidecar
/// wake and `k2 sidecar new` on a stopped name.
///
/// Every stored identity token is dropped first (a fresh sidecar's tab
/// row holds `--session-id <sid>`; replaying it starts a NEW conversation
/// or is refused), then the harness's own resume form is added: claude /
/// grok / gemini / cursor `--resume <sid>`, pi `--session <file>`, codex
/// `resume <sid>`, hermes `--resume <sid>`.
///
/// `check_disk` (CLI sidecars and the msg wake): a premint harness whose
/// transcript was never written (stopped before its first turn) gets the
/// same id minted again instead — `--resume` of a missing id exits.
/// Restart recovery of app-made tabs passes `false` (the long-standing
/// "always resume" contract). Unknown commands come back unchanged.
pub fn resume_argv_with(
    command: &str,
    saved_args: &[String],
    sid: &str,
    project_path: &str,
    check_disk: bool,
) -> Vec<String> {
    let sid = sid.trim();
    let Some(adapter) = provider_resume_for_command(command) else {
        return saved_args.to_vec();
    };
    if sid.is_empty() {
        return saved_args.to_vec();
    }
    let base = strip_session_identity(command, saved_args);
    match identity_style(adapter.provider) {
        Some(IdentityStyle::Premint(flag))
            if check_disk && !conversation_exists(adapter.provider, sid, project_path) =>
        {
            let mut args = base;
            args.push(flag.to_string());
            args.push(sid.to_string());
            args
        }
        Some(IdentityStyle::SessionFile) => {
            let mut args = base;
            args.push("--session".to_string());
            args.push(sid.to_string());
            args
        }
        _ => adapter.resume_args(&base, sid),
    }
}

/// [`resume_argv_with`] with the disk check on — sidecars and the msg wake.
pub fn resume_argv(
    command: &str,
    saved_args: &[String],
    sid: &str,
    project_path: &str,
) -> Vec<String> {
    resume_argv_with(command, saved_args, sid, project_path, true)
}

// ── Brief (§6.4) ─────────────────────────────────────────────────────

/// Workspace-relative brief path.
pub fn brief_rel_path(slug: &str) -> String {
    format!(".k2/sidecars/{slug}/BRIEF.md")
}

/// Size and text checks (SC9): ≤ 64 KiB, no NUL. (UTF-8 is guaranteed by
/// the caller holding a `&str`; byte bodies are checked by the route.)
pub fn check_brief(text: &str) -> Result<(), SidecarError> {
    if text.len() > BRIEF_MAX_BYTES {
        return Err(SidecarError::new(
            "brief_too_large",
            413,
            format!("briefs are at most {} KiB", BRIEF_MAX_BYTES / 1024),
        ));
    }
    if text.contains('\0') {
        return Err(SidecarError::new(
            "brief_not_text",
            400,
            "the brief has a NUL byte",
        ));
    }
    Ok(())
}

/// Byte count and the first 12 hex digits of the brief's SHA-256 — the
/// only things logs and audit ever carry (SC10).
pub fn brief_digest(text: &str) -> (usize, String) {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(text.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    (text.len(), hex[..12].to_string())
}

/// Written brief facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefInfo {
    pub rel_path: String,
    pub bytes: usize,
    pub sha12: String,
}

/// Write `.k2/sidecars/<slug>/BRIEF.md` atomically with mode 0600. The
/// first write also drops `.k2/sidecars/.gitignore` (`*`), so briefs are
/// never committed in a workspace that tracks `.k2/`.
pub fn write_brief(project_path: &Path, slug: &str, text: &str) -> Result<BriefInfo, String> {
    check_brief(text).map_err(|e| e.hint)?;
    let root = project_path.join(".k2").join("sidecars");
    let dir = root.join(slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let ignore = root.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n").map_err(|e| format!("write {}: {e}", ignore.display()))?;
    }
    let target = dir.join("BRIEF.md");
    let tmp = dir.join(format!(".BRIEF.md.{}.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| format!("write brief: {e}"))?;
        f.write_all(text.as_bytes()).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("write brief: {e}")
        })?;
        f.sync_all().ok();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod brief: {e}"))?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename brief: {e}")
    })?;
    let (bytes, sha12) = brief_digest(text);
    Ok(BriefInfo {
        rel_path: brief_rel_path(slug),
        bytes,
        sha12,
    })
}

/// The marker a sidecar's first turn starts with; discovery looks for it.
pub fn marker_for(address: &str) -> String {
    format!("[k2 sidecar] You are {address},")
}

/// The one-line first message of a new sidecar with a brief (§6.4).
pub fn pointer_message(address: &str, primary: &str, started_by: &str, brief_rel: &str) -> String {
    format!(
        "{} a sidecar of {primary}, started by {started_by}. Your brief is {brief_rel}. \
         Read it now and follow it. Re-read it when you lose the thread.",
        marker_for(address)
    )
}

/// First message when `new` resumes with a new brief (§6.3).
pub fn brief_changed_message(address: &str, brief_rel: &str) -> String {
    format!(
        "{} your brief changed. Read {brief_rel} now and follow it.",
        marker_for(address)
    )
}

/// First message of a fresh restart of a harness whose conversation was
/// never discovered (codex / hermes before their id is known).
pub fn restarted_message(address: &str, primary: &str, brief_rel: &str) -> String {
    format!(
        "{} a sidecar of {primary}. This is a fresh start; your earlier turns were not saved. \
         Your brief is {brief_rel}. Read it now and follow it.",
        marker_for(address)
    )
}

// ── Tab-row sidecar columns (migration 0132) ─────────────────────────

/// The sidecar columns of one tab row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarMeta {
    pub created_by: Option<String>,
    pub brief_path: Option<String>,
    pub adopt_since: Option<i64>,
}

/// Read the sidecar columns for `(project_id, pane_group_id)`.
pub fn meta(conn: &Connection, project_id: &str, pane_group_id: &str) -> Option<SidecarMeta> {
    conn.query_row(
        "SELECT created_by, brief_path, adopt_since FROM workspace_tab_sessions \
         WHERE project_id = ?1 AND pane_group_id = ?2",
        params![project_id, pane_group_id],
        |r| {
            Ok(SidecarMeta {
                created_by: r.get(0)?,
                brief_path: r.get(1)?,
                adopt_since: r.get(2)?,
            })
        },
    )
    .ok()
}

/// Write the sidecar columns (SC38). `None` leaves a column as it is.
pub fn stamp_meta(
    conn: &Connection,
    project_id: &str,
    pane_group_id: &str,
    created_by: Option<&str>,
    brief_path: Option<&str>,
    adopt_since: Option<i64>,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE workspace_tab_sessions SET \
                created_by = COALESCE(?3, created_by), \
                brief_path = COALESCE(?4, brief_path), \
                adopt_since = COALESCE(?5, adopt_since) \
             WHERE project_id = ?1 AND pane_group_id = ?2",
            params![
                project_id,
                pane_group_id,
                created_by,
                brief_path,
                adopt_since
            ],
        )
        .map_err(|e| format!("stamp sidecar row: {e}"))?;
    if n == 0 {
        return Err(format!("no tab row for pane group {pane_group_id}"));
    }
    Ok(())
}

// ── Read-only list (§5.3, SC43) ──────────────────────────────────────

/// One durable sidecar row (live state is added by the daemon).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarRow {
    pub pane_group_id: String,
    pub agent_name: String,
    pub handle: Option<String>,
    pub name: Option<String>,
    pub provider: &'static str,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Provider conversation id (pane key when not discovered yet).
    pub conversation_key: String,
    pub conversation_id: Option<String>,
    pub created_by: Option<String>,
    pub brief_path: Option<String>,
    pub adopt_since: Option<i64>,
    pub last_seen_at: i64,
}

/// Handle WITHOUT allocating: the Chats-name slug, else an existing
/// ordinal, else `None`.
pub fn handle_read_only(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Option<String> {
    if let Ok(Some(name)) = handles::custom_name_for_session_id(conn, conversation_key) {
        if let Ok(slug) = handles::slugify_custom_name(&name) {
            return Some(slug);
        }
    }
    handles::get(conn, project_id, conversation_key)
        .ok()
        .flatten()
        .map(|r| r.ordinal.to_string())
}

/// The workspace's sidecars: `tab-*` tab rows whose command is a Big-7
/// harness and whose conversation is not the pinned chat. Rows that share
/// one conversation fold into the newest. Never writes (SC43).
pub fn list_rows(conn: &Connection, project_id: &str) -> Result<Vec<SidecarRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at, \
                    created_by, brief_path, adopt_since \
             FROM workspace_tab_sessions WHERE project_id = ?1 \
             ORDER BY last_seen_at DESC, pane_group_id ASC",
        )
        .map_err(|e| e.to_string())?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
        Option<i64>,
    )> = stmt
        .query_map(params![project_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string())?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (
        pg,
        agent_name,
        sid,
        command,
        args_json,
        cwd,
        last_seen,
        created_by,
        brief_path,
        adopt_since,
    ) in rows
    {
        if !handles::is_tab_agent_name(&agent_name) {
            continue;
        }
        let Some(command) = command.filter(|c| !c.trim().is_empty()) else {
            continue;
        };
        let Some(adapter) = supported_provider(&command) else {
            continue;
        };
        let sid = sid.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(s) = sid.as_deref() {
            if handles::conversation_is_canonical(conn, project_id, s) {
                continue;
            }
        }
        let key = handles::conversation_key_for(sid.as_deref(), &pg);
        if !seen.insert(key.clone()) {
            continue;
        }
        let name = handles::custom_name_for_session_id(conn, &key)
            .ok()
            .flatten();
        out.push(SidecarRow {
            handle: handle_read_only(conn, project_id, &key),
            name,
            provider: adapter.provider,
            args: args_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
            command,
            cwd,
            conversation_key: key,
            conversation_id: sid,
            created_by,
            brief_path,
            adopt_since,
            last_seen_at: last_seen,
            pane_group_id: pg,
            agent_name,
        });
    }
    Ok(out)
}

// ── Saved layout (SC34) ──────────────────────────────────────────────

/// The workspace row the renderer saves the layout under (lowest
/// `tab_order`, the renderer's `primaryWorkspaceId`).
pub fn primary_workspace_id(conn: &Connection, project_id: &str) -> Option<String> {
    crate::db::schema::Workspace::list(conn, project_id)
        .ok()?
        .into_iter()
        .next()
        .map(|w| w.id)
}

/// Outcome of a daemon layout write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutWrite {
    /// Wrote a new revision.
    Wrote { workspace_id: String, revision: i64 },
    /// Nothing to do (the tab was already there / already gone).
    Unchanged { workspace_id: String },
    /// The project has no `workspaces` row, so no layout to write.
    NoWorkspace,
}

/// The layout tab id for a sidecar pane group (the renderer's adopt id).
pub fn adopted_tab_id(pane_group_id: &str) -> String {
    format!("adopted-{pane_group_id}")
}

fn tab_holds_pane(tab: &serde_json::Value, pg: &str) -> bool {
    tab.get("paneGroups")
        .and_then(|v| v.as_object())
        .is_some_and(|m| m.contains_key(pg))
}

fn layout_holds_pane(layout: &serde_json::Value, pg: &str) -> bool {
    let in_tabs = |tabs: Option<&serde_json::Value>| {
        tabs.and_then(|v| v.as_array())
            .is_some_and(|a| a.iter().any(|t| tab_holds_pane(t, pg)))
    };
    in_tabs(layout.get("tabs"))
        || layout
            .get("extraGroups")
            .and_then(|v| v.as_array())
            .is_some_and(|gs| gs.iter().any(|g| in_tabs(g.get("tabs"))))
}

/// A layout tab for a sidecar, the shape `serializeTab` writes.
pub fn layout_tab_json(
    pane_group_id: &str,
    title: &str,
    command: Option<&str>,
    conversation_id: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::json!({
        "id": format!("item-{pane_group_id}"),
        "type": "terminal",
        "paneGroupId": pane_group_id,
    });
    if let Some(c) = command.filter(|c| !c.trim().is_empty()) {
        item["commandHint"] = serde_json::Value::String(basename(c).to_string());
    }
    if let Some(cid) = conversation_id.filter(|c| !c.trim().is_empty()) {
        item["conversationId"] = serde_json::Value::String(cid.to_string());
    }
    let mut pgs = serde_json::Map::new();
    pgs.insert(
        pane_group_id.to_string(),
        serde_json::json!({ "id": pane_group_id, "items": [item], "activeItemIndex": 0 }),
    );
    serde_json::json!({
        "id": adopted_tab_id(pane_group_id),
        "title": title,
        "mosaicTree": pane_group_id,
        "paneGroups": serde_json::Value::Object(pgs),
        "locked": true,
    })
}

fn edit_layout<F>(project_id: &str, mut edit: F) -> Result<LayoutWrite, String>
where
    F: FnMut(&mut serde_json::Value) -> bool,
{
    let workspace_id = {
        let db = crate::db::shared();
        let conn = db.lock();
        primary_workspace_id(&conn, project_id)
    };
    let Some(workspace_id) = workspace_id else {
        return Ok(LayoutWrite::NoWorkspace);
    };
    for _attempt in 0..6 {
        let (stored, revision) =
            crate::db_ops::workspace_layout_load_with_revision(project_id, &workspace_id)?;
        let mut layout: serde_json::Value = match stored.as_deref() {
            Some(raw) if !raw.trim().is_empty() => {
                serde_json::from_str(raw).map_err(|e| format!("stored layout is not JSON: {e}"))?
            }
            _ => serde_json::json!({ "version": 2, "tabs": [] }),
        };
        if !layout.is_object() {
            layout = serde_json::json!({ "version": 2, "tabs": [] });
        }
        if !edit(&mut layout) {
            return Ok(LayoutWrite::Unchanged { workspace_id });
        }
        let json = serde_json::to_string(&layout).map_err(|e| e.to_string())?;
        let base = if stored.is_some() {
            Some(revision)
        } else {
            Some(0)
        };
        match crate::db_ops::workspace_layout_save_cas(project_id, &workspace_id, &json, base)? {
            crate::db_ops::LayoutSaveOutcome::Saved(rev) => {
                return Ok(LayoutWrite::Wrote {
                    workspace_id,
                    revision: rev,
                })
            }
            crate::db_ops::LayoutSaveOutcome::Conflict(_) => continue,
        }
    }
    Err("layout kept changing; gave up after 6 tries".to_string())
}

/// Make sure the saved layout has a tab for `pane_group_id` (SC34). Adds
/// `adopted-<pg>` (locked, titled) to the main strip when no tab holds
/// that pane group in the main strip or a split column.
pub fn ensure_layout_tab(
    project_id: &str,
    pane_group_id: &str,
    title: &str,
    command: Option<&str>,
    conversation_id: Option<&str>,
) -> Result<LayoutWrite, String> {
    edit_layout(project_id, |layout| {
        if layout_holds_pane(layout, pane_group_id) {
            return false;
        }
        let tab = layout_tab_json(pane_group_id, title, command, conversation_id);
        match layout.get_mut("tabs").and_then(|v| v.as_array_mut()) {
            Some(tabs) => tabs.push(tab),
            None => {
                layout["tabs"] = serde_json::Value::Array(vec![tab]);
            }
        }
        true
    })
}

/// Remove the tab that holds `pane_group_id` from the saved layout
/// (main strip and split columns), so an unattended visit does not
/// resume a stopped sidecar (§5.4 step 6). A tab holding other panes too
/// keeps them; only the pane group is removed from it.
pub fn remove_layout_tab(project_id: &str, pane_group_id: &str) -> Result<LayoutWrite, String> {
    edit_layout(project_id, |layout| {
        let mut changed = false;
        let mut strip = |tabs: Option<&mut serde_json::Value>| {
            let Some(tabs) = tabs.and_then(|v| v.as_array_mut()) else {
                return;
            };
            let before = tabs.len();
            tabs.retain(|t| {
                !(tab_holds_pane(t, pane_group_id)
                    && t.get("paneGroups")
                        .and_then(|v| v.as_object())
                        .is_some_and(|m| m.len() == 1))
            });
            if tabs.len() != before {
                changed = true;
            }
            for t in tabs.iter_mut() {
                if let Some(m) = t.get_mut("paneGroups").and_then(|v| v.as_object_mut()) {
                    if m.remove(pane_group_id).is_some() {
                        changed = true;
                    }
                }
            }
        };
        strip(layout.get_mut("tabs"));
        if let Some(groups) = layout.get_mut("extraGroups").and_then(|v| v.as_array_mut()) {
            for g in groups.iter_mut() {
                strip(g.get_mut("tabs"));
            }
        }
        changed
    })
}

// ── Discovery + adoption (codex, hermes) ─────────────────────────────

/// Every conversation id some tab row or pinned chat already owns.
pub fn claimed_conversation_ids(conn: &Connection) -> HashSet<String> {
    let mut out = HashSet::new();
    for sql in [
        "SELECT session_id FROM workspace_tab_sessions WHERE session_id IS NOT NULL",
        "SELECT session_id FROM workspace_sessions WHERE session_id IS NOT NULL",
    ] {
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                out.extend(
                    rows.flatten()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                );
            }
        }
    }
    out
}

/// Pick the conversation a codex / hermes sidecar started.
///
/// Candidates: conversations of `provider` whose cwd is `project_path`,
/// started at or after `since_secs`, not in `exclude` (ids that existed
/// before the spawn) and not claimed by any tab row or pinned chat.
/// - Exactly one candidate carrying the sidecar marker wins.
/// - Without a marker match, a lone candidate wins only when
///   `allow_unmarked` (the caller knows no other unadopted session of this
///   harness runs in the workspace, and the sidecar had no brief).
/// Anything else → `None` (keep waiting; never guess).
pub fn discover_conversation(
    provider: &str,
    project_path: &str,
    address: &str,
    since_secs: i64,
    exclude: &HashSet<String>,
    claimed: &HashSet<String>,
    allow_unmarked: bool,
) -> Option<String> {
    let marker = marker_for(address);
    let found = match provider {
        "codex" => crate::chat_history::codex_conversations_started_since(
            project_path,
            since_secs,
            Some(&marker),
        ),
        "hermes" => crate::chat_history::hermes_conversations_started_since(
            project_path,
            since_secs,
            Some(&marker),
        ),
        _ => return None,
    };
    let candidates: Vec<_> = found
        .into_iter()
        .filter(|c| !exclude.contains(&c.id) && !claimed.contains(&c.id))
        .collect();
    let marked: Vec<_> = candidates.iter().filter(|c| c.marked).collect();
    match marked.len() {
        1 => return Some(marked[0].id.clone()),
        0 => {}
        _ => return None,
    }
    if allow_unmarked && candidates.len() == 1 {
        return Some(candidates[0].id.clone());
    }
    None
}

/// Ids a self-minting harness already holds for this workspace, taken
/// right before a spawn so discovery never adopts an older conversation.
pub fn existing_conversation_ids(provider: &str, project_path: &str) -> HashSet<String> {
    let found = match provider {
        "codex" => crate::chat_history::codex_conversations_started_since(project_path, 0, None),
        "hermes" => crate::chat_history::hermes_conversations_started_since(project_path, 0, None),
        _ => Vec::new(),
    };
    found.into_iter().map(|c| c.id).collect()
}

/// Bind a discovered conversation `cid` to the sidecar in
/// `(project_id, pane_group_id)`: stamp the tab row (which rekeys the
/// handle row), move a pane-keyed Chats name onto `cid`, clear
/// `adopt_since`. Idempotent.
pub fn adopt_conversation(
    conn: &Connection,
    project_id: &str,
    pane_group_id: &str,
    provider: &str,
    cid: &str,
) -> Result<(), String> {
    let pane_name = handles::custom_name_for_session_id(conn, pane_group_id)?;
    if let Some(name) = pane_name {
        conn.execute(
            "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
             VALUES (?1, ?2, ?3, 0, unixepoch()) \
             ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?3, updated_at = unixepoch()",
            params![provider, cid, name],
        )
        .map_err(|e| format!("move chat name: {e}"))?;
        conn.execute(
            "DELETE FROM chat_session_names WHERE session_id = ?1",
            params![pane_group_id],
        )
        .map_err(|e| format!("move chat name: {e}"))?;
    }
    crate::db::schema::WorkspaceTabSession::stamp_session_id(conn, project_id, pane_group_id, cid)
        .map_err(|e| format!("stamp conversation: {e}"))?;
    conn.execute(
        "UPDATE workspace_tab_sessions SET adopt_since = NULL \
         WHERE project_id = ?1 AND pane_group_id = ?2",
        params![project_id, pane_group_id],
    )
    .map_err(|e| format!("clear adopt_since: {e}"))?;
    Ok(())
}

/// Bind a display name to a conversation key for THIS workspace (SC32):
/// workspace-only slug check, then the durable ordinal, then the Chats
/// name. `conversation_key` is the provider id, or the pane group id for
/// a harness whose id is discovered later.
pub fn bind_name(
    conn: &Connection,
    project_id: &str,
    provider: &str,
    conversation_key: &str,
    slug: &str,
    name: &str,
) -> Result<(), SidecarError> {
    handles::ensure_slug_unique_in_workspace(conn, project_id, conversation_key, slug)
        .map_err(|e| SidecarError::new("name_taken", 409, e))?;
    handles::allocate_ordinal(conn, project_id, conversation_key)
        .map_err(|e| SidecarError::new("name_taken", 409, e))?;
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES (?1, ?2, ?3, 0, unixepoch()) \
         ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?3, updated_at = unixepoch()",
        params![provider, conversation_key, name.trim()],
    )
    .map_err(|e| SidecarError::new("name_taken", 409, format!("save name: {e}")))?;
    Ok(())
}

/// Provider stored with a conversation's Chats name (SC40).
pub fn named_provider(conn: &Connection, conversation_key: &str) -> Option<String> {
    conn.query_row(
        "SELECT provider FROM chat_session_names WHERE session_id = ?1 \
         AND TRIM(custom_name) != '' ORDER BY updated_at DESC LIMIT 1",
        params![conversation_key],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_follow_the_address_rules() {
        assert_eq!(validate_name("Gardens", "k2").unwrap(), "gardens");
        assert_eq!(
            validate_name("Thread Polish", "k2").unwrap(),
            "thread-polish"
        );
        let e = validate_name("Gardens & Apps", "k2").unwrap_err();
        assert_eq!(e.code, "bad_name");
        assert!(e.hint.contains("gardens-and-apps"), "{}", e.hint);
        assert_eq!(validate_name("5", "k2").unwrap_err().code, "bad_name");
        assert_eq!(validate_name("123", "k2").unwrap_err().code, "bad_name");
        assert_eq!(validate_name("k2", "k2").unwrap_err().code, "bad_name");
        assert_eq!(validate_name("", "k2").unwrap_err().code, "bad_name");
        assert_eq!(
            validate_name(&"a".repeat(41), "k2").unwrap_err().code,
            "bad_name"
        );
        assert_eq!(
            validate_name(&"a".repeat(40), "k2").unwrap(),
            "a".repeat(40)
        );
        assert_eq!(validate_name("a/b", "k2").unwrap_err().code, "bad_name");
        assert_eq!(
            validate_name("snake_case", "k2").unwrap_err().code,
            "bad_name"
        );
    }

    #[test]
    fn identity_styles_cover_the_big_seven() {
        assert_eq!(
            identity_style("claude"),
            Some(IdentityStyle::Premint("--session-id"))
        );
        assert_eq!(
            identity_style("grok"),
            Some(IdentityStyle::Premint("--session-id"))
        );
        assert_eq!(
            identity_style("gemini"),
            Some(IdentityStyle::Premint("--session-id"))
        );
        assert_eq!(identity_style("pi"), Some(IdentityStyle::SessionFile));
        assert_eq!(identity_style("cursor"), Some(IdentityStyle::CreateChat));
        assert_eq!(identity_style("codex"), Some(IdentityStyle::Discover));
        assert_eq!(identity_style("hermes"), Some(IdentityStyle::Discover));
        assert_eq!(identity_style("aider"), None);
        for p in SUPPORTED_PROVIDERS {
            assert!(identity_style(p).is_some(), "{p}");
        }
    }

    #[test]
    fn strip_session_identity_drops_every_grammar() {
        assert_eq!(
            strip_session_identity(
                "claude",
                &a(&["--dangerously-skip-permissions", "--session-id", "X"])
            ),
            a(&["--dangerously-skip-permissions"])
        );
        assert_eq!(
            strip_session_identity("claude", &a(&["--resume", "X", "--model", "opus"])),
            a(&["--model", "opus"])
        );
        assert_eq!(
            strip_session_identity("codex", &a(&["--yolo", "resume", "X"])),
            a(&["--yolo"])
        );
        assert_eq!(
            strip_session_identity("pi", &a(&["--session", "/p/x.jsonl"])),
            a(&[])
        );
        assert_eq!(
            strip_session_identity("cursor-agent", &a(&["--resume", "C", "--force"])),
            a(&["--force"])
        );
        assert_eq!(
            strip_session_identity("gemini", &a(&["--yolo", "--session-id", "G"])),
            a(&["--yolo"])
        );
        assert_eq!(
            strip_session_identity("bash", &a(&["--resume", "X"])),
            a(&["--resume", "X"])
        );
    }

    #[test]
    fn skip_approval_detection() {
        assert!(carries_skip_approvals(
            &a(&["--dangerously-skip-permissions"]),
            &[]
        ));
        assert!(carries_skip_approvals(
            &a(&["--approval-mode", "yolo"]),
            &[]
        ));
        assert!(carries_skip_approvals(&a(&["--custom"]), &a(&["--custom"])));
        assert!(!carries_skip_approvals(&a(&["--model", "x"]), &[]));
    }

    #[test]
    fn brief_checks_and_digest() {
        assert!(check_brief(&"x".repeat(BRIEF_MAX_BYTES)).is_ok());
        let big = check_brief(&"x".repeat(BRIEF_MAX_BYTES + 1)).unwrap_err();
        assert_eq!((big.code, big.status), ("brief_too_large", 413));
        assert_eq!(check_brief("a\0b").unwrap_err().code, "brief_not_text");
        let (n, sha) = brief_digest("hello");
        assert_eq!(n, 5);
        assert_eq!(sha, "2cf24dba5fb0");
    }

    #[cfg(unix)]
    #[test]
    fn write_brief_is_0600_atomic_and_gitignored() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("k2-sidecar-brief-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let info = write_brief(&dir, "gardens", "# Brief\nDo the thing.\n").unwrap();
        assert_eq!(info.rel_path, ".k2/sidecars/gardens/BRIEF.md");
        let path = dir.join(&info.rel_path);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Brief\nDo the thing.\n"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "brief mode {mode:o}");
        assert_eq!(
            std::fs::read_to_string(dir.join(".k2/sidecars/.gitignore")).unwrap(),
            "*\n"
        );
        // Replacing keeps 0600 and leaves no temp files behind.
        write_brief(&dir, "gardens", "v2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        let leftovers: Vec<_> = std::fs::read_dir(dir.join(".k2/sidecars/gardens"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["BRIEF.md".to_string()]);
        assert!(write_brief(&dir, "big", &"x".repeat(BRIEF_MAX_BYTES + 1)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pointer_line_is_one_line_and_carries_the_marker() {
        let p = pointer_message(
            "k2/gardens",
            "k2",
            "k2 (agent)",
            ".k2/sidecars/gardens/BRIEF.md",
        );
        assert!(!p.contains('\n'));
        assert!(p.starts_with(&marker_for("k2/gardens")));
        assert!(p.contains(".k2/sidecars/gardens/BRIEF.md"));
        assert!(restarted_message("k2/x", "k2", "b").starts_with(&marker_for("k2/x")));
    }

    #[test]
    fn layout_tab_shape_and_pane_lookup() {
        let tab = layout_tab_json("pg1", "Gardens", Some("/usr/bin/claude"), Some("cid"));
        assert_eq!(tab["id"], "adopted-pg1");
        assert_eq!(tab["title"], "Gardens");
        assert_eq!(tab["locked"], true);
        assert_eq!(
            tab["paneGroups"]["pg1"]["items"][0]["commandHint"],
            "claude"
        );
        assert_eq!(
            tab["paneGroups"]["pg1"]["items"][0]["conversationId"],
            "cid"
        );
        let layout = serde_json::json!({"tabs": [], "extraGroups": [{"tabs": [tab]}]});
        assert!(layout_holds_pane(&layout, "pg1"));
        assert!(!layout_holds_pane(&layout, "pg2"));
    }

    // ── Per-harness resume argv (SC48 helper; Big 7) ─────────────────

    struct Home {
        dir: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: parking_lot::MutexGuard<'static, ()>,
    }
    impl Home {
        fn new(tag: &str) -> Self {
            let lock = crate::themes::HOME_LOCK.lock();
            let dir = std::env::temp_dir()
                .join(format!("k2-sidecar-resume-{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", &dir);
            Self {
                dir,
                prev,
                _lock: lock,
            }
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const PROJECT: &str = "/Users/fixture/sidecar-proj";
    const CID: &str = "11111111-2222-4333-8444-555555555555";

    #[test]
    fn resume_claude_drops_premint_and_resumes_when_the_transcript_exists() {
        let home = Home::new("claude");
        let saved = a(&["--dangerously-skip-permissions", "--session-id", CID]);
        // Nothing on disk yet: the same id is minted again (resume would fail).
        assert_eq!(
            resume_argv("claude", &saved, CID, PROJECT),
            a(&["--dangerously-skip-permissions", "--session-id", CID])
        );
        let dir = home
            .dir
            .join(".claude/projects")
            .join(crate::chat_history::claude_project_hash(PROJECT));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{CID}.jsonl")), "{}\n").unwrap();
        assert_eq!(
            resume_argv("claude", &saved, CID, PROJECT),
            a(&["--dangerously-skip-permissions", "--resume", CID])
        );
    }

    #[test]
    fn resume_grok_resumes_when_the_session_exists() {
        let home = Home::new("grok");
        let saved = a(&["--always-approve", "--session-id", CID]);
        assert_eq!(resume_argv("grok", &saved, CID, PROJECT), saved);
        let dir = home.dir.join(".grok/sessions/%2Ffixture").join(CID);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("summary.json"),
            serde_json::json!({"info": {"id": CID, "cwd": PROJECT}, "last_active_at": "2026-10-07T00:00:00Z"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            resume_argv("grok", &saved, CID, PROJECT),
            a(&["--always-approve", "--resume", CID])
        );
    }

    #[test]
    fn resume_gemini_resumes_when_a_chat_file_holds_the_id() {
        let home = Home::new("gemini");
        let saved = a(&["--yolo", "--session-id", CID]);
        assert_eq!(resume_argv("gemini", &saved, CID, PROJECT), saved);
        let dir = home.dir.join(".gemini/tmp/slug/chats");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("session-2026-10-07T00-00-{}.jsonl", &CID[..8])),
            format!("{{\"sessionId\":\"{CID}\"}}\n"),
        )
        .unwrap();
        assert_eq!(
            resume_argv("gemini", &saved, CID, PROJECT),
            a(&["--yolo", "--resume", CID])
        );
    }

    #[test]
    fn resume_pi_reopens_its_session_file() {
        let _home = Home::new("pi");
        let file = "/home/u/.pi/agent/sessions/--x--/2026_abc.jsonl";
        assert_eq!(
            resume_argv("pi", &a(&["--session", file]), file, PROJECT),
            a(&["--session", file])
        );
        let new_path = new_pi_session_path("/Users/a/b").unwrap();
        assert!(
            new_path
                .to_string_lossy()
                .contains("/.pi/agent/sessions/--Users-a-b--/"),
            "{}",
            new_path.display()
        );
    }

    #[test]
    fn resume_cursor_resumes_its_chat() {
        assert_eq!(
            resume_argv(
                "cursor-agent",
                &a(&["--force", "--resume", "chat-1"]),
                "chat-1",
                PROJECT
            ),
            a(&["--force", "--resume", "chat-1"])
        );
    }

    #[test]
    fn resume_codex_uses_the_subcommand() {
        assert_eq!(
            resume_argv("codex", &a(&["--yolo"]), "019a-codex", PROJECT),
            a(&["--yolo", "resume", "019a-codex"])
        );
        assert_eq!(
            resume_argv(
                "codex",
                &a(&["--yolo", "resume", "old"]),
                "019a-codex",
                PROJECT
            ),
            a(&["--yolo", "resume", "019a-codex"])
        );
    }

    #[test]
    fn resume_hermes_uses_resume_flag() {
        assert_eq!(
            resume_argv("hermes", &a(&[]), "20261007_abc", PROJECT),
            a(&["--resume", "20261007_abc"])
        );
    }

    #[test]
    fn resume_without_disk_check_always_resumes() {
        let _home = Home::new("strict");
        // App-tab restart recovery: drop the premint, resume, no disk probe.
        assert_eq!(
            resume_argv_with(
                "claude",
                &a(&["--dangerously-skip-permissions", "--session-id", CID]),
                CID,
                PROJECT,
                false
            ),
            a(&["--dangerously-skip-permissions", "--resume", CID])
        );
        assert_eq!(
            resume_argv_with("gemini", &a(&["--session-id", CID]), CID, PROJECT, false),
            a(&["--resume", CID])
        );
    }

    #[test]
    fn resume_unknown_command_is_unchanged() {
        assert_eq!(
            resume_argv("aider", &a(&["--x"]), CID, PROJECT),
            a(&["--x"])
        );
    }

    #[test]
    fn suggest_slug_examples() {
        assert_eq!(suggest_slug("Gardens & Apps"), "gardens-and-apps");
        assert_eq!(suggest_slug("snake_case name"), "snake-case-name");
    }
}
