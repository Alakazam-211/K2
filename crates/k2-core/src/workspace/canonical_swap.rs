//! Prepare a canonical-chat swap.
//!
//! The pinned tab keeps resuming `workspace_sessions` even after the
//! workspace default agent changes. [`prepare_canonical_swap`] does not
//! spawn and does not retarget that row. It writes the handoff the next
//! chat should read, and returns the text the daemon sends after
//! [`super::resume_chat::resolve_never_chatted_chat_args`] has started a
//! new session of the target harness.
//!
//! The transcript seed is the same read-only text as
//! `POST /cli/chat/continue-seed`. Extra notes are appended. Claude's
//! pinned argv is not involved: the handoff is a file plus a later
//! message, never a new flag.

use std::fs;
use std::path::Path;

use super::agent_identity::workspace_agent_path;
use crate::chat_continue::{
    build_continue_seed, ContinueMode, ContinueSeedRequest, CONTINUE_FRAMING,
};
use crate::workspace::provider_resume::{
    provider_resume_for_command, provider_resume_for_provider,
};

/// File name under the workspace agent dir (`.k2/agent/` or `.k2so/agent/`).
pub const HANDOFF_FILE_NAME: &str = "CANONICAL-HANDOFF.md";

/// Extra notes are capped so a swap request cannot hand the new chat a
/// multi-megabyte paste. The transcript seed has its own caps.
const NOTES_CHAR_CAP: usize = 32_000;

/// What the daemon needs after a successful prepare. The session row is
/// unchanged.
#[derive(Debug, Clone)]
pub struct CanonicalSwapPrep {
    pub project_path: String,
    pub target_provider: String,
    pub from_provider: String,
    pub from_session_id: String,
    pub handoff_path: String,
    pub message: String,
    /// Set when the previous transcript could not be copied. The swap
    /// still has a message: the fallback plus any notes.
    pub seed_error: Option<String>,
}

/// Build and store the handoff. `mode` is `recent` or `full`.
///
/// `Err` on an unknown harness, a bad mode, an unregistered project, or
/// a handoff write failure. A missing transcript is not an error.
pub fn prepare_canonical_swap(
    project_path: &str,
    target_provider: &str,
    mode: &str,
    extra_notes: &str,
) -> Result<CanonicalSwapPrep, String> {
    let project_path = project_path.trim();
    if project_path.is_empty() {
        return Err("project required".to_string());
    }
    let target = normalize_target(target_provider)?;
    let mode = parse_mode(mode)?;
    let notes = cap_notes(extra_notes);

    let (from_provider, from_session_id) = saved_canonical(project_path)?;
    let (seed, seed_error) = match (&from_provider, &from_session_id) {
        (provider, session) if !provider.is_empty() && !session.is_empty() => {
            match build_continue_seed(&ContinueSeedRequest {
                provider: provider.clone(),
                session_id: session.clone(),
                project_path: project_path.to_string(),
                mode,
            }) {
                Ok(text) => (Some(text), None),
                Err(err) => (None, Some(err)),
            }
        }
        _ => (None, Some("no saved canonical session".to_string())),
    };

    let handoff_path = workspace_agent_path(project_path).join(HANDOFF_FILE_NAME);
    let handoff_display = handoff_path.display().to_string();
    let message = compose_message(
        seed.as_deref(),
        seed_error.as_deref(),
        &from_provider,
        &from_session_id,
        project_path,
        &notes,
        &handoff_display,
    );
    write_handoff(&handoff_path, &message)?;

    Ok(CanonicalSwapPrep {
        project_path: project_path.to_string(),
        target_provider: target.to_string(),
        from_provider,
        from_session_id,
        handoff_path: handoff_display,
        message,
        seed_error,
    })
}

fn normalize_target(raw: &str) -> Result<&'static str, String> {
    let token = raw.trim().split_whitespace().next().unwrap_or("");
    let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let lower = base.to_ascii_lowercase();
    if lower.is_empty() {
        return Err(
            "provider required (claude, grok, cursor, gemini, pi, codex, or hermes)".to_string(),
        );
    }
    if let Some(adapter) = provider_resume_for_provider(&lower) {
        return Ok(adapter.provider);
    }
    if let Some(adapter) = provider_resume_for_command(&lower) {
        return Ok(adapter.provider);
    }
    Err(format!(
        "unknown harness: {raw} (expected claude, grok, cursor, gemini, pi, codex, or hermes)"
    ))
}

fn parse_mode(mode: &str) -> Result<ContinueMode, String> {
    match mode.trim() {
        "" | "recent" => Ok(ContinueMode::Recent),
        "full" => Ok(ContinueMode::Full),
        other => Err(format!("mode must be recent or full (got {other})")),
    }
}

fn saved_canonical(project_path: &str) -> Result<(String, String), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| format!("project not registered: {project_path}"))?;
    let row = crate::db::schema::WorkspaceSession::get(&conn, &project_id)
        .map_err(|err| format!("read canonical session: {err}"))?
        .filter(|row| row.session_id.as_deref().is_some_and(|id| !id.is_empty()));
    Ok(match row {
        Some(row) => (row.harness, row.session_id.unwrap_or_default()),
        None => (String::new(), String::new()),
    })
}

fn cap_notes(notes: &str) -> String {
    let trimmed = notes.trim();
    if trimmed.chars().count() <= NOTES_CHAR_CAP {
        return trimmed.to_string();
    }
    let kept: String = trimmed.chars().take(NOTES_CHAR_CAP).collect();
    format!("{kept}\n[notes truncated]")
}

fn compose_message(
    seed: Option<&str>,
    seed_error: Option<&str>,
    from_provider: &str,
    from_session_id: &str,
    project_path: &str,
    notes: &str,
    handoff_path: &str,
) -> String {
    let mut message = String::new();
    if let Some(seed) = seed {
        message.push_str(seed);
        if !seed.ends_with('\n') {
            message.push('\n');
        }
    } else {
        message.push_str(CONTINUE_FRAMING);
        message.push_str("\n\nWorkspace: ");
        message.push_str(project_path);
        message.push('\n');
        if from_session_id.is_empty() {
            message.push_str("No earlier canonical session was saved.\n");
        } else {
            message.push_str("Previous canonical chat: ");
            message.push_str(if from_provider.is_empty() {
                "unknown"
            } else {
                from_provider
            });
            message.push(' ');
            message.push_str(from_session_id);
            message.push('\n');
            if let Some(err) = seed_error {
                message.push_str("Its transcript was not copied (");
                message.push_str(err);
                message.push_str(").\n");
            }
        }
        message.push_str("Read ROLE.md before you act.\n");
    }
    if !notes.is_empty() {
        message.push_str("\nNotes passed with the swap:\n");
        message.push_str(notes);
        message.push('\n');
    }
    message.push_str("\nHandoff file: ");
    message.push_str(handoff_path);
    message.push('\n');
    message
}

fn write_handoff(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create handoff dir: {err}"))?;
    }
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, body).map_err(|err| format!("write handoff: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("save handoff: {err}")
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        _lock: parking_lot::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(label: &str) -> (Self, PathBuf) {
            let lock = crate::themes::HOME_LOCK.lock();
            let home = std::env::temp_dir().join(format!(
                "k2-swap-{label}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&home).unwrap();
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", &home);
            (
                Self {
                    original,
                    _lock: lock,
                },
                home,
            )
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn insert_project(path: &str) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, default_agent) VALUES (?1, 'test', ?2, NULL)",
            rusqlite::params![id, path],
        )
        .expect("insert project");
        id
    }

    fn set_saved_session(project_id: &str, session_id: &str, harness: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        let row_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'user', 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET session_id = ?3, harness = ?4",
            rusqlite::params![row_id, project_id, session_id, harness],
        )
        .unwrap();
    }

    fn saved_row(project_id: &str) -> Option<(Option<String>, String)> {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::db::schema::WorkspaceSession::get(&conn, project_id)
            .unwrap()
            .map(|row| (row.session_id, row.harness))
    }

    #[test]
    fn unknown_harness_does_not_write_or_require_a_project() {
        let err = prepare_canonical_swap("/tmp/does-not-matter", "aider", "recent", "notes")
            .expect_err("unknown");
        assert!(err.contains("unknown harness"), "got: {err}");
    }

    #[test]
    fn bad_mode_is_rejected() {
        let err = prepare_canonical_swap("/tmp/does-not-matter", "claude", "everything", "")
            .expect_err("mode");
        assert!(err.contains("mode must be recent or full"), "got: {err}");
    }

    #[test]
    fn missing_session_writes_fallback_and_notes_without_a_session_row() {
        let (_guard, home) = HomeGuard::new("fallback");
        crate::db::init_for_tests();
        let project = home.join("ws");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.to_string_lossy().to_string();
        let project_id = insert_project(&path);

        let prep = prepare_canonical_swap(&path, "Claude", "recent", "Keep the role notes.")
            .expect("prep");
        assert_eq!(prep.target_provider, "claude");
        assert!(prep.from_session_id.is_empty());
        assert_eq!(
            prep.seed_error.as_deref(),
            Some("no saved canonical session")
        );
        assert!(prep.message.contains(CONTINUE_FRAMING));
        assert!(prep.message.contains("Keep the role notes."));
        assert!(prep.message.contains("Read ROLE.md"));
        let on_disk = std::fs::read_to_string(&prep.handoff_path).expect("handoff");
        assert_eq!(on_disk, prep.message);
        assert!(
            saved_row(&project_id).is_none(),
            "prepare must not insert a session"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn grok_transcript_is_copied_and_the_saved_row_stays() {
        let (_guard, home) = HomeGuard::new("grok-seed");
        crate::db::init_for_tests();
        let project = home.join("ws");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.to_string_lossy().to_string();
        let project_id = insert_project(&path);
        let session_id = "01920000-eeee-7000-8000-0000000000aa";
        set_saved_session(&project_id, session_id, "grok");
        let session_dir = home
            .join(".grok")
            .join("sessions")
            .join("encoded")
            .join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": session_id, "cwd": path },
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            session_dir.join("chat_history.jsonl"),
            "{\"type\":\"user\",\"content\":\"keep the role notes\"}\n\
             {\"type\":\"assistant\",\"content\":\"the file stays\"}\n",
        )
        .unwrap();

        let prep = prepare_canonical_swap(&path, "cursor-agent", "recent", "").expect("prep");
        assert_eq!(prep.target_provider, "cursor");
        assert_eq!(prep.from_provider, "grok");
        assert_eq!(prep.from_session_id, session_id);
        assert!(
            prep.seed_error.is_none(),
            "seed error: {:?}",
            prep.seed_error
        );
        assert!(prep.message.contains("keep the role notes"));
        assert!(prep.message.contains("the file stays"));
        assert!(prep.message.contains(CONTINUE_FRAMING));
        assert!(!prep.message.contains("--resume"));
        assert_eq!(
            saved_row(&project_id),
            Some((Some(session_id.to_string()), "grok".to_string())),
            "prepare must not retarget the canonical row"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn long_notes_are_capped() {
        let (_guard, home) = HomeGuard::new("cap");
        crate::db::init_for_tests();
        let project = home.join("ws");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.to_string_lossy().to_string();
        insert_project(&path);
        let notes = "n".repeat(NOTES_CHAR_CAP + 40);
        let prep = prepare_canonical_swap(&path, "claude", "full", &notes).expect("prep");
        assert!(prep.message.contains("[notes truncated]"));
        assert!(!prep.message.contains(&"n".repeat(NOTES_CHAR_CAP + 40)));
        let _ = std::fs::remove_dir_all(&home);
    }
}
