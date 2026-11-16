//! Switch one agent session's model, live, without changing anyone's
//! default (0.46.13, Rosson).
//!
//! - `POST /cli/sessions/model` `{session_id, model, thread?}`: K2 types
//!   `/model <id>` into that Claude session, the same locked inject the
//!   composer's `/compact` and `/goal` use. No restart: the conversation and
//!   its running subagents carry on. Claude Code also saves that pick as the
//!   default for every new session (its user settings file), so K2 waits for
//!   that write (up to [`SAVE_WAIT`]) and puts the `model` key back
//!   ([`k2_core::cli_model_settings`]): to the server's default for Claude
//!   (Settings → Providers / Models), else to what it was. Then a K2 note on
//!   the Thread: "Model: … for this chat; your default stays …".
//!   Only at the agent's prompt: mid-turn (or waiting on a person) is
//!   refused, nothing sent. A person's move: the owner, a Connect Owner or
//!   Admin (the route floor), or a person typing in a K2 shell tab; agents
//!   and App guests are refused.
//! - `GET /cli/sessions/model/options?session_id=`: the session's harness,
//!   whether it can switch, the model it is on (when K2 knows), and the
//!   server and workspace defaults, for the Thread's `/model` menu.
//!
//! Which settings file: the session's own Claude home. A wallet login's
//! session runs with `CLAUDE_CONFIG_DIR` = its slot (noted at spawn,
//! [`note_spawn_home`]; after a daemon restart, the home K2 recorded for the
//! session key); everything else uses the daemon's live home
//! (`~/.claude`, or the daemon's own `CLAUDE_CONFIG_DIR`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use k2_core::activity::Display;
use k2_core::cli_model_settings::{self as cms, Restore};
use k2_core::session::SessionId;
use k2_core::sidecar::SidecarError;
use serde_json::{json, Value};

use crate::sidecar_routes::{err, Caller};

pub const ROUTE: &str = "/cli/sessions/model";
pub const OPTIONS_ROUTE: &str = "/cli/sessions/model/options";

/// How long K2 waits for Claude to save the pick before giving up on the
/// restore (nothing is written then).
pub const SAVE_WAIT: Duration = Duration::from_secs(10);
const SAVE_POLL: Duration = Duration::from_millis(100);

/// Session id → the `CLAUDE_CONFIG_DIR` its CLI started with (`None` = the
/// live home).
static HOMES: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Session id → the model K2 last switched it to.
static CURRENT: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Note which Claude home a just-spawned session runs in (both PTY spawn
/// doors call this with the final env). Other harnesses are not noted.
pub fn note_spawn_home(session_id: &SessionId, program: Option<&str>, env: &HashMap<String, String>) {
    let Some(program) = program else { return };
    if k2_core::workspace::model_splice::binary_token(program) != "claude" {
        return;
    }
    let home = env.get("CLAUDE_CONFIG_DIR").map(|s| s.trim()).filter(|s| !s.is_empty()).map(PathBuf::from);
    lock(&HOMES).insert(session_id.to_string(), home);
}

/// The Claude home a live session's CLI writes its settings to.
fn claude_home(session_id: &SessionId, map_key: Option<&str>) -> PathBuf {
    let live = || k2_core::llm_accounts::store::live_home(k2_core::llm_accounts::Tool::Claude).0;
    if let Some(noted) = lock(&HOMES).get(&session_id.to_string()) {
        return noted.clone().unwrap_or_else(live);
    }
    if let Some(key) = map_key {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Ok(Some(home)) =
            k2_core::llm_accounts::pins::recorded_home_for_key(&conn, k2_core::llm_accounts::Tool::Claude, key)
        {
            return home;
        }
    }
    live()
}

/// Who may switch an agent's model: a person (owner token, a Connect login
/// past the route's Admin floor, a K2 shell tab). Agents and App guests no.
pub(crate) fn gate(caller: &Caller) -> Result<(), SidecarError> {
    match caller {
        Caller::Owner | Caller::Login { .. } | Caller::Shell { .. } => Ok(()),
        Caller::Agent(_) => Err(err(
            "model_switch_needs_person",
            403,
            "only a person switches an agent's model: from the Thread's / menu (/model), or the owner from a terminal",
        )),
        Caller::WrongCredential => Err(err(
            "wrong_credential",
            403,
            "app passes, API keys and API sessions can't switch an agent's model",
        )),
        Caller::Invalid => Err(err("forbidden", 403, "invalid or missing token")),
    }
}

/// Refuse while the agent is mid-turn or waiting on a person: `/model`
/// typed then would land in its turn. Idle, monitoring (its subagents work
/// on, the lead is at its prompt) and unknown are fine.
pub(crate) fn busy_refusal(display: Option<Display>, who: &str) -> Result<(), SidecarError> {
    let why = match display {
        Some(Display::Working) => "is mid-turn",
        Some(Display::Waiting) => "is waiting on a person",
        _ => return Ok(()),
    };
    Err(err(
        "agent_busy",
        409,
        format!("{who} {why}, and K2 switches the model only at its prompt so its turn isn't cut into. Try again when it's done. Nothing was sent."),
    ))
}

/// The switch once the session is known: busy check, then the inject,
/// then the wait-and-restore on `settings`. `inject` types the command.
/// The outer `Err` is a refusal (nothing sent); the inner one a restore
/// that failed after the switch went through.
pub(crate) fn switch_live(
    display: Option<Display>,
    who: &str,
    pick: &str,
    settings: &Path,
    default: Option<&str>,
    wait: Duration,
    inject: impl FnOnce(&str) -> Result<(), &'static str>,
) -> Result<Result<Restore, String>, SidecarError> {
    busy_refusal(display, who)?;
    let before = cms::snapshot(settings);
    inject(&format!("/model {pick}")).map_err(|code| {
        err(code, 409, format!("the /model command didn't reach the session ({code}); nothing was switched"))
    })?;
    Ok(cms::wait_and_restore(settings, &before, pick, default, wait, SAVE_POLL))
}

/// The Thread note for a switch.
pub(crate) fn note_text(pick: &str, restore: &Result<Restore, String>) -> String {
    let head = format!("Model: {pick} for this chat");
    match restore {
        Ok(Restore::Restored { to: Some(m) }) => format!("{head}; your default stays {m}."),
        Ok(Restore::Restored { to: None }) => format!("{head}; your default stays Claude's own (none saved)."),
        Ok(Restore::Kept { model }) => format!("{head}; your default stays {model}."),
        Ok(Restore::AlreadyPick) => format!("{head}; your default stays {pick} (it already was)."),
        Ok(Restore::ChangedElsewhere { model }) => format!(
            "{head}. Your saved default changed to {} by something else meanwhile, so K2 left it.",
            model.as_deref().unwrap_or("nothing")
        ),
        Ok(Restore::NotWritten) => format!("{head}; Claude didn't save it as your default."),
        Err(e) => format!("{head}, but K2 couldn't put your default back: {e}"),
    }
}

fn restore_json(restore: &Result<Restore, String>) -> Value {
    match restore {
        Ok(Restore::Restored { to }) => json!({ "outcome": "restored", "default": to }),
        Ok(Restore::Kept { model }) => json!({ "outcome": "kept", "default": model }),
        Ok(Restore::AlreadyPick) => json!({ "outcome": "already_pick" }),
        Ok(Restore::ChangedElsewhere { model }) => json!({ "outcome": "changed_elsewhere", "default": model }),
        Ok(Restore::NotWritten) => json!({ "outcome": "not_written" }),
        Err(e) => json!({ "outcome": "failed", "error": e }),
    }
}

struct Live {
    sid: SessionId,
    map_key: Option<String>,
    program: String,
    args: Vec<String>,
    cwd: String,
}

fn live_session(raw: Option<&str>) -> Result<Live, SidecarError> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| err("bad_request", 400, "session_id required"))?;
    let sid = SessionId::parse(raw).ok_or_else(|| err("bad_request", 400, "session_id is not a session id"))?;
    let session = crate::v2_session_map::lookup_by_session_id(&sid)
        .filter(|s| s.is_child_alive())
        .ok_or_else(|| err("no_session", 404, format!("no live agent session {raw}")))?;
    let map_key = crate::v2_session_map::snapshot()
        .into_iter()
        .find(|(_, s)| s.session_id == sid)
        .map(|(k, _)| k);
    Ok(Live {
        sid,
        map_key,
        program: session.program.clone().unwrap_or_default(),
        args: session.args.clone(),
        cwd: session.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    })
}

fn harness_of(program: &str) -> &str {
    k2_core::workspace::model_splice::binary_token(program)
}

/// The model the session is on, when K2 knows: its last K2 switch, else
/// the model its argv names.
fn current_model(live: &Live) -> Option<String> {
    if let Some(m) = lock(&CURRENT).get(&live.sid.to_string()) {
        return Some(m.clone());
    }
    let flags = k2_core::workspace::model_splice::existing_model_flag_indices(&live.args);
    let f = flags.last()?;
    let a = &live.args[f.index];
    match a.strip_prefix("--model=") {
        Some(v) => Some(v.to_string()),
        None => live.args.get(f.index + 1).cloned(),
    }
}

fn project_of(cwd: &str) -> Option<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = k2_core::workspace::agent_identity::resolve_project_id(&conn, cwd)?;
    Some((id, cwd.to_string()))
}

/// `GET /cli/sessions/model/options`.
pub(crate) fn handle_options(caller: &Caller, params: &HashMap<String, String>) -> Result<Value, SidecarError> {
    gate(caller)?;
    let live = live_session(params.get("session_id").map(String::as_str))?;
    let harness = harness_of(&live.program).to_string();
    let workspace_default = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::model_splice::load_workspace_model(&conn, &live.cwd).0
    };
    Ok(json!({
        "ok": true,
        "sessionId": live.sid.to_string(),
        "harness": harness,
        "switchable": harness == "claude",
        "current": current_model(&live),
        "serverDefault": k2_core::app_settings::server_default_model(&live.program),
        "workspaceDefault": workspace_default,
    }))
}

/// `POST /cli/sessions/model`.
pub(crate) fn handle_switch(caller: &Caller, body: &Value) -> Result<Value, SidecarError> {
    gate(caller)?;
    let pick = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err("bad_request", 400, "model required (for example opus)"))?
        .to_string();
    if !k2_core::app_settings::valid_model_id(&pick) {
        return Err(err("bad_model", 400, format!("{pick:?} is not a model id")));
    }
    let live = live_session(body.get("session_id").and_then(Value::as_str))?;
    if harness_of(&live.program) != "claude" {
        return Err(err(
            "harness_unsupported",
            409,
            format!(
                "switching the model live works for Claude sessions; this one runs {}",
                if live.program.is_empty() { "a shell" } else { harness_of(&live.program) }
            ),
        ));
    }
    let project = project_of(&live.cwd);
    let who = "The agent";
    let display = crate::activity_events::displays().get(&live.sid.to_string()).map(|(d, _)| *d);
    let settings = cms::settings_path(&claude_home(&live.sid, live.map_key.as_deref()));
    let default = k2_core::app_settings::server_default_model(&live.program);
    let sid = live.sid;
    let restore = switch_live(display, who, &pick, &settings, default.as_deref(), SAVE_WAIT, |cmd| {
        crate::workspace_msg::inject_cli_command(&sid, cmd)
    })?;
    lock(&CURRENT).insert(live.sid.to_string(), pick.clone());
    let text = note_text(&pick, &restore);
    let noted = post_note(body.get("thread").and_then(Value::as_str), project.as_ref(), &text);
    k2_core::log_debug!("[sessions-model] session={} model={pick} restore={restore:?} noted={noted}", live.sid);
    Ok(json!({
        "ok": true,
        "sessionId": live.sid.to_string(),
        "model": pick,
        "default": restore_json(&restore),
        "note": text,
        "noted": noted,
    }))
}

/// The note goes on the Thread the client named when it is this session's
/// workspace's, else on the workspace's own Thread.
fn post_note(thread: Option<&str>, project: Option<&(String, String)>, text: &str) -> bool {
    let Some((project_id, path)) = project else { return false };
    let addr = thread
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter(|t| crate::overlay_routes::thread_project_of(t).as_deref() == Some(project_id.as_str()))
        .unwrap_or(path.as_str());
    match crate::overlay_routes::post_k2_note(addr, text, "k2-model") {
        Ok(_) => true,
        Err(e) => {
            k2_core::log_debug!("[sessions-model] note not posted on {addr}: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const FILE: &str = "{\n  \"model\": \"sonnet\",\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  },\n  \"theme\": \"dark\"\n}\n";

    fn temp_settings(tag: &str) -> (PathBuf, PathBuf) {
        let home = std::env::temp_dir().join(format!("k2-sm-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let path = cms::settings_path(&home);
        std::fs::write(&path, FILE).unwrap();
        (home, path)
    }

    /// Mid-turn (or waiting on a person): refused with a clear line, the
    /// command is never typed, the settings file is untouched.
    #[test]
    fn refused_while_mid_turn_and_nothing_is_sent() {
        let (home, path) = temp_settings("busy");
        for d in [Display::Working, Display::Waiting] {
            let typed = Cell::new(false);
            let e = switch_live(Some(d), "ws/1", "haiku", &path, Some("opus"), Duration::from_millis(100), |_| {
                typed.set(true);
                Ok(())
            })
            .expect_err("busy is refused");
            assert_eq!((e.code, e.status), ("agent_busy", 409));
            assert!(e.hint.contains("Nothing was sent"), "{}", e.hint);
            assert!(!typed.get(), "the command is never typed while busy");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// At its prompt: K2 types `/model <pick>`; Claude saves the pick; K2
    /// puts the key back to the server default, and only that key changes.
    #[test]
    fn switch_types_the_command_then_restores_only_the_model_key() {
        let (home, path) = temp_settings("ok");
        for d in [None, Some(Display::Idle), Some(Display::Monitoring)] {
            std::fs::write(&path, FILE).unwrap();
            let typed = Cell::new(String::new());
            let p = path.clone();
            let r = switch_live(d, "ws/1", "haiku", &path, Some("opus"), Duration::from_secs(5), |cmd| {
                typed.set(cmd.to_string());
                // Claude saves the pick a moment later.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    let raw = std::fs::read_to_string(&p).unwrap();
                    std::fs::write(&p, raw.replace("\"sonnet\"", "\"haiku\"")).unwrap();
                });
                Ok(())
            })
            .expect("switched");
            assert_eq!(typed.take(), "/model haiku");
            assert_eq!(r, Ok(Restore::Restored { to: Some("opus".into()) }));
            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, FILE.replace("\"sonnet\"", "\"opus\""), "only the model value changed");
            assert_eq!(note_text("haiku", &r), "Model: haiku for this chat; your default stays opus.");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The file changed underneath to something that isn't K2's pick: left
    /// alone, and the note says so.
    #[test]
    fn no_restore_when_the_file_changed_underneath() {
        let (home, path) = temp_settings("else");
        let p = path.clone();
        let r = switch_live(None, "ws/1", "haiku", &path, None, Duration::from_millis(400), |_| {
            let raw = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, raw.replace("\"sonnet\"", "\"fable\"")).unwrap();
            Ok(())
        })
        .expect("switched");
        assert_eq!(r, Ok(Restore::ChangedElsewhere { model: Some("fable".into()) }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE.replace("\"sonnet\"", "\"fable\""));
        assert!(note_text("haiku", &r).contains("K2 left it"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_failed_inject_is_a_refusal_and_writes_nothing() {
        let (home, path) = temp_settings("dead");
        let e = switch_live(None, "ws/1", "haiku", &path, Some("opus"), Duration::from_millis(100), |_| Err("pty_died"))
            .expect_err("refused");
        assert_eq!((e.code, e.status), ("pty_died", 409));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn only_people_may_switch() {
        assert!(gate(&Caller::Owner).is_ok());
        assert!(gate(&Caller::Login { username: "anna".into() }).is_ok());
        assert!(gate(&Caller::Shell { project_id: "p".into() }).is_ok());
        let agent = Caller::Agent(crate::sidecar_routes::AgentCaller {
            project_id: "p".into(),
            map_key: None,
            args: vec![],
            sandboxed: false,
        });
        assert_eq!(gate(&agent).unwrap_err().code, "model_switch_needs_person");
        assert_eq!(gate(&Caller::WrongCredential).unwrap_err().code, "wrong_credential");
        assert_eq!(gate(&Caller::Invalid).unwrap_err().code, "forbidden");
        // The agent is refused before anything is looked up or sent.
        let e = handle_switch(&agent, &json!({"session_id": "x", "model": "opus"})).unwrap_err();
        assert_eq!(e.code, "model_switch_needs_person");
    }

    #[test]
    fn switch_checks_the_model_and_the_session() {
        let e = handle_switch(&Caller::Owner, &json!({"session_id": "x"})).unwrap_err();
        assert_eq!((e.code, e.status), ("bad_request", 400));
        let e = handle_switch(&Caller::Owner, &json!({"session_id": "x", "model": "opus; rm"})).unwrap_err();
        assert_eq!((e.code, e.status), ("bad_model", 400));
        let e = handle_switch(&Caller::Owner, &json!({"session_id": "x", "model": "-p"})).unwrap_err();
        assert_eq!(e.code, "bad_model");
        let gone = SessionId::new().to_string();
        let e = handle_switch(&Caller::Owner, &json!({"session_id": gone, "model": "opus"})).unwrap_err();
        assert_eq!((e.code, e.status), ("no_session", 404));
    }
}
