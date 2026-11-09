//! Where a tool keeps its conversations: one answer for every reader.
//!
//! A chat is not always in the tool's normal home. A chat pinned to a
//! wallet login runs with that login's slot as its home (Claude and Grok
//! keep their history there), and a preset can point the tool somewhere
//! else with its own env (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `GROK_HOME`).
//! The session list, the resume checks and the transcript readers all go
//! through [`roots`] and [`files`], so they agree on what exists.
//!
//! Bounded: one `read_dir` of the tool's wallet folder and one small query.
//! No transcript is opened here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::llm_accounts::{pins, store, Tool};

/// A preset's own home for this tool (its `env` holds the tool's home
/// variable). A path K2 can't read as absolute is ignored: the program
/// would resolve it against its own cwd, not against anything K2 knows.
fn preset_homes(tool: Tool) -> Vec<PathBuf> {
    let Some(db) = crate::db::try_shared() else {
        return Vec::new();
    };
    let var = tool.home_env_var();
    let raw: Vec<String> = {
        let conn = db.lock();
        let Ok(mut st) = conn.prepare("SELECT env FROM agent_presets WHERE env LIKE '%' || ?1 || '%'") else {
            return Vec::new();
        };
        st.query_map([var], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    };
    raw.into_iter()
        .filter_map(|json| {
            let env: serde_json::Value = serde_json::from_str(&json).ok()?;
            let value = env.get(var)?.as_str()?.trim().to_string();
            match value.strip_prefix("~/") {
                Some(rest) => Some(crate::paths::home_dir()?.join(rest)),
                None => Some(PathBuf::from(value)).filter(|p| p.is_absolute()),
            }
        })
        .collect()
}

/// Every home that can hold this tool's conversations, the live home
/// first: the daemon's live home, the tool's default home, each wallet
/// login's slot, and any preset's own home.
pub fn homes(tool: Tool) -> Vec<PathBuf> {
    let mut out = vec![store::live_home(tool).0];
    if let Some(home) = crate::paths::home_dir() {
        out.push(home.join(format!(".{}", tool.as_str())));
    }
    if let Ok(entries) = std::fs::read_dir(store::tool_dir(tool)) {
        let mut slots: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        slots.sort();
        out.extend(slots);
    }
    out.extend(preset_homes(tool));
    out
}

/// One path per real location: a Codex shadow slot's linked `sessions`
/// folder is the normal one, and must not be walked twice.
fn unique_real(paths: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .filter(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())))
        .collect()
}

/// The `<home>/<sub>` folders that exist for this tool (`projects` for
/// Claude, `sessions` for Codex and Grok), the live home's first.
pub fn roots(tool: Tool, sub: &str) -> Vec<PathBuf> {
    unique_real(homes(tool).into_iter().map(|h| h.join(sub)).filter(|p| p.is_dir()))
}

/// The `<home>/<name>` files that exist for this tool (`history.jsonl`).
pub fn files(tool: Tool, name: &str) -> Vec<PathBuf> {
    unique_real(homes(tool).into_iter().map(|h| h.join(name)).filter(|p| p.is_file()))
}

/// Does this home hold the conversation in a project folder?
fn holds(home: &Path, root: &str, conversation_id: &str) -> bool {
    let Ok(dirs) = std::fs::read_dir(home.join(root)) else {
        return false;
    };
    let jsonl = format!("{conversation_id}.jsonl");
    dirs.flatten().any(|d| {
        d.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && [jsonl.as_str(), conversation_id]
                .iter()
                .any(|name| std::fs::symlink_metadata(d.path().join(name)).is_ok())
    })
}

/// The home to bring a resumed conversation from: `None` when `to` already
/// has it or nothing else does. For a resume whose login record names no
/// other home (the record can be missing or out of date, and a resume in a
/// home without the history fails with "no conversation found").
pub fn conversation_home_elsewhere(tool: Tool, conversation_id: &str, to: &Path) -> Option<PathBuf> {
    let root = pins::conversation_root(tool)?;
    if holds(to, root, conversation_id) {
        return None;
    }
    homes(tool).into_iter().find(|h| h != to && holds(h, root, conversation_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_continue::read_transcript_messages;
    use crate::chat_history as ch;
    use crate::test_env::TempHome;

    fn claude_file(tool_home: &Path, project: &str, sid: &str) -> PathBuf {
        let dir = tool_home.join("projects").join(ch::claude_project_hash(project));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{sid}.jsonl"));
        std::fs::write(
            &file,
            serde_json::json!({"type":"user","cwd":project,"message":{"role":"user","content":"hello there"},"timestamp":"2026-10-01T00:00:00Z"})
                .to_string()
                + "\n",
        )
        .unwrap();
        file
    }

    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        std::fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    /// The reported bug: the main chat exists only in an account's folder.
    /// K2 must see it everywhere it lists or reads chats.
    #[test]
    fn a_chat_only_in_an_account_folder_is_found() {
        let _home = TempHome::new();
        crate::db::init_for_tests();
        let project = "/fixture/homes-account-only";
        let sid = "aaaaaaaa-1111-4111-8111-111111111111";
        let slot = store::slot_dir(Tool::Claude, "acct-a");
        let file = claude_file(&slot, project, sid);

        assert!(ch::claude_session_file_exists(sid, project));
        assert_eq!(ch::resolve_claude_session_file(sid, project).as_deref(), Some(file.as_path()));
        assert_eq!(ch::newest_claude_session_on_disk(project).as_deref(), Some(sid));
        let listed = ch::parse_claude_sessions(Some(project)).expect("parse");
        assert_eq!(listed.iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), vec![sid]);
        let digest = read_transcript_messages("claude", sid, project).expect("show reads the account's copy");
        assert_eq!(digest.messages.len(), 1);
        assert_eq!(ch::resolve_session_storage_path("claude", sid, project).as_deref(), file.to_str());
    }

    /// Two copies: `show`/`resume` read the one written last, and the list
    /// shows the real last-active time, not the stale copy's.
    #[test]
    fn the_newest_copy_across_homes_wins_and_sets_last_active() {
        let home = TempHome::new();
        crate::db::init_for_tests();
        let project = "/fixture/homes-two-copies";
        let sid = "bbbbbbbb-2222-4222-8222-222222222222";
        let stale = claude_file(&home.path().join(".claude"), project, sid);
        let stale_at = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        set_mtime(&stale, stale_at);
        let fresh = claude_file(&store::slot_dir(Tool::Claude, "acct-a"), project, sid);

        assert_eq!(ch::resolve_claude_session_file(sid, project).as_deref(), Some(fresh.as_path()));
        let rows = ch::parse_claude_sessions(Some(project)).expect("parse");
        assert_eq!(rows.len(), 1, "one row per conversation: {rows:?}");
        let stale_ms = stale_at.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!(rows[0].timestamp > stale_ms + 86_400_000, "last active is the fresh copy's: {rows:?}");
    }

    /// A chat launched with a preset's own `CODEX_HOME`: K2 learns its id,
    /// lists it, reads it and can resume it.
    #[test]
    fn a_custom_codex_home_chat_is_found() {
        let home = TempHome::new();
        crate::db::init_for_tests();
        let custom = home.path().join("my-codex");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in, env) \
                 VALUES (?1, 'custom codex', 'codex', '', 1, 960, 0, ?2)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    serde_json::json!({"CODEX_HOME": custom.to_string_lossy()}).to_string()
                ],
            )
            .unwrap();
        }
        let project = "/fixture/homes-custom-codex";
        let sid = "019a0000-0000-7000-8000-000000000001";
        let day = custom.join("sessions/2026/11/05");
        std::fs::create_dir_all(&day).unwrap();
        let line = serde_json::json!({"type":"session_meta","payload":{"id":sid,"cwd":project,"timestamp":"2026-11-05T00:00:00Z"}}).to_string();
        std::fs::write(day.join("rollout-2026-11-05T00-00-00-x.jsonl"), line + "\n").unwrap();

        assert_eq!(ch::detect_codex_session(project).as_deref(), Some(sid));
        let rows = ch::parse_codex_sessions(Some(project)).expect("parse");
        assert_eq!(rows.iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), vec![sid]);
        let adapter = crate::workspace::provider_resume::provider_resume_for_provider("codex").unwrap();
        assert!(adapter.session_file_exists(sid, project), "the resolver sees it, so a restart resumes it");
        assert!(read_transcript_messages("codex", sid, project).is_ok());
    }

    #[test]
    fn a_grok_chat_only_in_an_account_folder_is_found() {
        let _home = TempHome::new();
        crate::db::init_for_tests();
        let project = "/fixture/homes-grok";
        let sid = "019a0000-0000-7000-8000-00000000000a";
        let dir = store::slot_dir(Tool::Grok, "acct-g").join("sessions").join("%2Ffixture").join(sid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("summary.json"),
            serde_json::json!({"info":{"id":sid,"cwd":project},"last_active_at":"2026-07-03T10:00:00Z"}).to_string(),
        )
        .unwrap();
        assert!(ch::grok_session_file_exists(sid, project));
        assert_eq!(ch::newest_grok_session_on_disk(project).as_deref(), Some(sid));
    }

    /// Codex shadow slots link `sessions` to the live one: one walk only.
    #[cfg(unix)]
    #[test]
    fn a_linked_sessions_folder_counts_once() {
        let home = TempHome::new();
        crate::db::init_for_tests();
        std::fs::create_dir_all(home.path().join(".codex/sessions")).unwrap();
        let slot = store::slot_dir(Tool::Codex, "acct-c");
        std::fs::create_dir_all(&slot).unwrap();
        std::os::unix::fs::symlink(home.path().join(".codex/sessions"), slot.join("sessions")).unwrap();
        assert_eq!(roots(Tool::Codex, "sessions").len(), 1);
    }

    #[test]
    fn a_resume_finds_the_home_that_holds_the_history() {
        let home = TempHome::new();
        crate::db::init_for_tests();
        let project = "/fixture/homes-carry";
        let sid = "cccccccc-3333-4333-8333-333333333333";
        let slot = store::slot_dir(Tool::Claude, "acct-a");
        claude_file(&slot, project, sid);
        let live = home.path().join(".claude");
        std::fs::create_dir_all(&live).unwrap();
        assert_eq!(conversation_home_elsewhere(Tool::Claude, sid, &live), Some(slot.clone()));
        assert_eq!(conversation_home_elsewhere(Tool::Claude, sid, &slot), None, "already there");
    }
}
