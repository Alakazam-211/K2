//! Past conversations in one workspace (`k2 sessions --past`, `show`;
//! prd-workspace-sessions-cli-v1 WS-P / WS-S / WS-A5).
//!
//! Rows come from the seven Chat-history readers
//! ([`crate::chat_history`]), called one by one so that:
//! - no 100-row cap applies before the day and limit filters (WS-P5);
//! - a reader that fails is reported in `unsupported` with its reason,
//!   never dropped silently (§6);
//! - only conversations whose recorded cwd is the workspace root or under
//!   `<root>/.worktrees/` count, and never one that belongs to a nested
//!   registered project (WS-P2/P3).
//!
//! Search (WS-P4) matches title and custom name first, then the
//! user/assistant text of each transcript, newest first, inside a budget
//! of files, bytes and time. Over budget the answer says `truncated`.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::chat_continue::{self, TranscriptDigest};
use crate::chat_history::{self, ChatSession};

/// The seven CLIs Chat history reads.
pub const CLIS: &[&str] = &["claude", "codex", "grok", "gemini", "cursor", "pi", "hermes"];

/// Search budget (WS-P4).
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_time: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_files: 400,
            max_bytes: 64 * 1024 * 1024,
            max_time: Duration::from_secs(5),
        }
    }
}

/// What to list.
#[derive(Debug, Clone)]
pub struct PastQuery {
    /// The workspace root.
    pub root: String,
    /// Every registered project path (a nested one owns its own chats).
    pub project_paths: Vec<String>,
    /// Newer than this many days (1..=365).
    pub days: u32,
    pub cli: Option<String>,
    pub limit: usize,
    pub search: Option<String>,
    pub include_archived: bool,
    pub budget: Budget,
    /// "now" in unix seconds (tests pin it).
    pub now: i64,
}

/// One past conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct PastRow {
    pub conversation_id: String,
    pub cli: String,
    pub title: String,
    pub custom_name: Option<String>,
    pub last_active_at: i64,
    pub messages: usize,
    pub cwd: String,
    /// `.worktrees/<x>` relative to the root (`(gone)` appended when the
    /// folder no longer exists); `None` at the root.
    pub worktree: Option<String>,
    pub archived: bool,
    pub excerpt: Option<String>,
}

/// The answer.
#[derive(Debug, Clone, Default)]
pub struct PastResult {
    pub rows: Vec<PastRow>,
    pub truncated: bool,
    pub scanned_files: usize,
    pub scanned_bytes: u64,
    pub unsupported: Vec<(String, String)>,
}

fn secs(ts: i64) -> i64 {
    if ts > 100_000_000_000 {
        ts / 1000
    } else {
        ts
    }
}

/// The longest registered project path that holds `cwd`.
fn owner_project<'a>(cwd: &str, project_paths: &'a [String]) -> Option<&'a str> {
    let cwd = cwd.trim_end_matches('/');
    project_paths
        .iter()
        .map(|p| p.trim_end_matches('/'))
        .filter(|p| !p.is_empty() && (cwd == *p || cwd.starts_with(&format!("{p}/"))))
        .max_by_key(|p| p.len())
}

/// Does a conversation recorded at `cwd` belong to the workspace `root`?
/// The root itself or a `.worktrees/` folder under it, and not a nested
/// registered project (whose own `.worktrees/` stay its own).
pub fn belongs_to(cwd: &str, root: &str, project_paths: &[String]) -> bool {
    let cwd = cwd.trim_end_matches('/');
    let root = root.trim_end_matches('/');
    if cwd.is_empty() || !chat_history::matches_project_family(cwd, root) {
        return false;
    }
    let family_root = chat_history::resolve_root_project_path(cwd);
    match owner_project(family_root, project_paths) {
        Some(owner) => owner == root,
        None => true,
    }
}

fn worktree_of(cwd: &str, root: &str) -> Option<String> {
    let rest = cwd.trim_end_matches('/').strip_prefix(root.trim_end_matches('/'))?;
    let rest = rest.strip_prefix('/')?;
    if !rest.starts_with(".worktrees/") {
        return None;
    }
    if Path::new(cwd).is_dir() {
        Some(rest.to_string())
    } else {
        Some(format!("{rest} (gone)"))
    }
}

fn read_cli(cli: &str, root: &str, since_ms: i64, unsupported: &mut Vec<(String, String)>) -> Vec<ChatSession> {
    let filter = Some(root);
    let res = match cli {
        "claude" => chat_history::parse_claude_sessions(filter),
        "cursor" => chat_history::parse_cursor_sessions(filter),
        "gemini" => chat_history::parse_gemini_sessions(filter),
        "pi" => chat_history::parse_pi_sessions(filter),
        "codex" => {
            let mut unreadable = 0usize;
            let r = chat_history::parse_codex_sessions_since(filter, Some(since_ms), &mut unreadable);
            if unreadable > 0 {
                unsupported.push((
                    "codex".into(),
                    format!("{unreadable} codex rollout file(s) could not be read (no session_meta header)"),
                ));
            }
            r
        }
        "grok" => chat_history::parse_grok_sessions(filter),
        "hermes" => chat_history::parse_hermes_sessions(filter),
        _ => Ok(Vec::new()),
    };
    match res {
        Ok(rows) => rows,
        Err(e) => {
            unsupported.push((cli.to_string(), format!("could not read {cli} chats: {e}")));
            Vec::new()
        }
    }
}

fn cursor_present() -> bool {
    crate::paths::home_dir().is_some_and(|h| h.join(".cursor").join("chats").is_dir())
}

/// A ≤ 160-character excerpt around the first case-insensitive match.
pub fn excerpt(text: &str, needle_lower: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let at = lower.find(needle_lower)?;
    // Byte offsets of the lowercase copy match the original for ASCII; for
    // other text fall back to the char index of the match.
    let char_at = lower[..at].chars().count();
    let chars: Vec<char> = text.chars().collect();
    let start = char_at.saturating_sub(60);
    let end = (char_at + needle_lower.chars().count() + 100).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > 156 {
        out = out.chars().take(156).collect();
    }
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < chars.len() { "…" } else { "" };
    Some(format!("{prefix}{out}{suffix}"))
}

/// List past conversations.
pub fn list_past(q: &PastQuery) -> PastResult {
    let days = q.days.clamp(1, 365) as i64;
    let since = q.now - days * 86_400;
    let mut out = PastResult::default();
    let clis: Vec<&str> = match q.cli.as_deref() {
        Some(c) => CLIS.iter().copied().filter(|x| *x == c).collect(),
        None => CLIS.to_vec(),
    };
    let mut rows: Vec<PastRow> = Vec::new();
    let names = chat_history::get_custom_names().unwrap_or_default();
    for cli in clis {
        if cli == "cursor" && cursor_present() {
            out.unsupported.push((
                "cursor".into(),
                "chats started in a worktree are not found (cursor keys chats by the root folder only); no message text for show or search".into(),
            ));
        }
        for s in read_cli(cli, &q.root, since * 1000, &mut out.unsupported) {
            let last = secs(s.timestamp);
            if last < since {
                continue;
            }
            if !belongs_to(&s.project, &q.root, &q.project_paths) {
                continue;
            }
            let custom = names
                .get(&format!("{}:{}", s.provider, s.session_id))
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty());
            rows.push(PastRow {
                worktree: worktree_of(&s.project, &q.root),
                conversation_id: s.session_id,
                cli: s.provider,
                title: s.title,
                custom_name: custom,
                last_active_at: last,
                messages: s.message_count,
                cwd: s.project,
                archived: s.archived,
                excerpt: None,
            });
        }
    }
    let archived: std::collections::HashSet<(String, String)> =
        crate::chat_user_archive::list_archived_rows(Some(&q.root))
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.provider, r.session_id))
            .collect();
    for r in rows.iter_mut() {
        r.archived = r.archived || archived.contains(&(r.cli.clone(), r.conversation_id.clone()));
    }
    if !q.include_archived {
        rows.retain(|r| !r.archived);
    }
    rows.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at).then_with(|| a.conversation_id.cmp(&b.conversation_id)));
    // One row per (cli, id): a reader may list a conversation twice.
    let mut seen = std::collections::HashSet::new();
    rows.retain(|r| seen.insert((r.cli.clone(), r.conversation_id.clone())));

    let limit = q.limit.clamp(1, 200);
    let needle = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase);
    let Some(needle) = needle else {
        rows.truncate(limit);
        out.rows = rows;
        return out;
    };
    let started = Instant::now();
    let mut hits = Vec::new();
    for mut r in rows {
        if hits.len() >= limit {
            break;
        }
        let in_title = r.title.to_lowercase().contains(&needle)
            || r.custom_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&needle));
        if in_title {
            r.excerpt = excerpt(r.custom_name.as_deref().unwrap_or(&r.title), &needle)
                .or_else(|| excerpt(&r.title, &needle));
            hits.push(r);
            continue;
        }
        if r.cli == "cursor" {
            continue;
        }
        if out.scanned_files >= q.budget.max_files
            || out.scanned_bytes >= q.budget.max_bytes
            || started.elapsed() >= q.budget.max_time
        {
            out.truncated = true;
            break;
        }
        let Ok(TranscriptDigest { messages, bytes, .. }) =
            chat_continue::read_transcript_messages(&r.cli, &r.conversation_id, &r.cwd)
        else {
            continue;
        };
        out.scanned_files += 1;
        out.scanned_bytes += bytes;
        if let Some(m) = messages.iter().find(|m| m.text.to_lowercase().contains(&needle)) {
            r.excerpt = excerpt(&m.text, &needle);
            hits.push(r);
        }
    }
    out.rows = hits;
    out
}

/// The shortest unique prefix (≥ 8 chars) of each id within `ids`
/// (WS-A5).
pub fn short_ids(ids: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for id in ids {
        let mut n = 8.min(id.len());
        while n < id.len() && ids.iter().any(|o| o != id && o.starts_with(&id[..n])) {
            n += 1;
        }
        out.insert(id.clone(), id[..n].to_string());
    }
    out
}

/// How an `<id>` argument resolved against a set of ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdMatch {
    One(String),
    None,
    /// Up to eight candidates.
    Ambiguous(Vec<String>),
    /// A prefix under six characters.
    TooShort,
}

/// Resolve a full id or a unique prefix of at least 6 characters.
pub fn match_id<'a>(token: &str, ids: impl IntoIterator<Item = &'a String>) -> IdMatch {
    let token = token.trim();
    let ids: Vec<&String> = ids.into_iter().collect();
    if let Some(exact) = ids.iter().find(|i| i.as_str() == token) {
        return IdMatch::One((*exact).clone());
    }
    if token.chars().count() < 6 {
        return IdMatch::TooShort;
    }
    let mut hits: Vec<String> = ids.iter().filter(|i| i.starts_with(token)).map(|i| (*i).clone()).collect();
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => IdMatch::None,
        1 => IdMatch::One(hits.remove(0)),
        _ => {
            hits.truncate(8);
            IdMatch::Ambiguous(hits)
        }
    }
}

/// The git branch of a folder (`None` if it is gone or not a repo).
pub fn branch_of(cwd: &str) -> Option<String> {
    if !Path::new(cwd).is_dir() {
        return None;
    }
    let out = std::process::Command::new("git")
        .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty()).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn belongs_to_root_and_worktrees_only() {
        let pp = paths(&["/x/K2", "/x/K2-other", "/x/K2/apps/nested"]);
        assert!(belongs_to("/x/K2", "/x/K2", &pp));
        assert!(belongs_to("/x/K2/", "/x/K2", &pp));
        assert!(belongs_to("/x/K2/.worktrees/k2-fix", "/x/K2", &pp));
        assert!(!belongs_to("/x/K2-other", "/x/K2", &pp), "a sibling folder is not this workspace");
        assert!(!belongs_to("/x/K2/apps/nested", "/x/K2", &pp), "a nested registered project owns its chats");
        assert!(!belongs_to("/x/K2/apps/nested/.worktrees/a", "/x/K2", &pp));
        assert!(!belongs_to("/x/K2/src", "/x/K2", &pp), "only the root or .worktrees (Chat history's rule)");
        assert!(!belongs_to("", "/x/K2", &pp));
    }

    #[test]
    fn short_ids_are_unique_prefixes_of_eight_or_more() {
        let ids = paths(&["4e9a77c2-aaaa", "4e9a77c2-bbbb", "8c1f2a9e-cccc"]);
        let s = short_ids(&ids);
        assert_eq!(s["8c1f2a9e-cccc"], "8c1f2a9e");
        assert_eq!(s["4e9a77c2-aaaa"], "4e9a77c2-a");
        assert_eq!(s["4e9a77c2-bbbb"], "4e9a77c2-b");
    }

    #[test]
    fn match_id_rules() {
        let ids = paths(&["4e9a77c2-aaaa", "4e9a77c2-bbbb", "8c1f2a9e-cccc"]);
        assert_eq!(match_id("8c1f2a", &ids), IdMatch::One("8c1f2a9e-cccc".into()));
        assert_eq!(match_id("8c1f2a9e-cccc", &ids), IdMatch::One("8c1f2a9e-cccc".into()));
        assert_eq!(match_id("8c1f", &ids), IdMatch::TooShort);
        assert_eq!(match_id("ffffff", &ids), IdMatch::None);
        assert_eq!(
            match_id("4e9a77", &ids),
            IdMatch::Ambiguous(vec!["4e9a77c2-aaaa".into(), "4e9a77c2-bbbb".into()])
        );
    }

    #[test]
    fn excerpt_is_bounded_and_centered() {
        let text = format!("{} the Thread snaps to the top {}", "a ".repeat(200), "b ".repeat(200));
        let e = excerpt(&text, "snaps").expect("found");
        assert!(e.contains("snaps"), "{e}");
        assert!(e.chars().count() <= 160, "{}", e.chars().count());
        assert_eq!(excerpt("nothing here", "snaps"), None);
    }

    #[test]
    fn worktree_names_relative_to_root() {
        assert_eq!(worktree_of("/x/K2", "/x/K2"), None);
        assert_eq!(
            worktree_of("/definitely/not/here/K2/.worktrees/k2-x", "/definitely/not/here/K2").as_deref(),
            Some(".worktrees/k2-x (gone)")
        );
    }
}
