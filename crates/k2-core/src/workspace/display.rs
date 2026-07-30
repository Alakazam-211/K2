//! Agent display name — the human-friendly label for the workspace's
//! primary agent.
//!
//! ## Source of truth
//!
//! `<project>/.k2so/agent/AGENT.md` frontmatter — first the optional
//! `display_name:` field, then the `name:` field, then the workspace's
//! `projects.name`. Returning a string is total: every workspace has a
//! `projects.name` (NOT NULL by schema) so we always have something to
//! show.
//!
//! Why two fields (`display_name:` AND `name:`)? Because `name:` is
//! today's *technical* identifier — it's what `find_primary_agent`
//! returns, what keys the v2_session_map, what stamps the
//! `workspace_sessions.terminal_id`. Renaming `name:` cascades through
//! every infrastructure layer; that's the rabbit hole 0.37.4 explicitly
//! avoids. `display_name:` is the user-editable label that decouples
//! "what shows on the inbox tab header" from "what keys the live PTY
//! map." A future 0.38.0 refactor can drop the technical name from
//! infrastructure entirely (the `agent-display-name.md` PRD) and
//! collapse the two fields into one — but that's a separate ship.
//!
//! ## Cache
//!
//! Display name reads happen on every render of the inbox tab + chat
//! tab title. Reading + parsing AGENT.md per render would be wasteful;
//! we keep an in-memory cache keyed on `(project_path, AGENT.md mtime)`
//! and invalidate on mtime change. External edits (vim, AIFileEditor)
//! get picked up on the next read because mtime moved.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::workspace::agent_identity::{parse_frontmatter, workspace_agent_md_path};

/// Cache entry: the resolved display name + the AGENT.md mtime it was
/// derived from. A `None` mtime means AGENT.md was missing at read
/// time, in which case we cached the workspace-name fallback.
#[derive(Clone)]
struct Cached {
    display_name: String,
    agent_md_mtime: Option<SystemTime>,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn current_mtime(path: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Resolve the agent's display name for a workspace. Total — always
/// returns a string. Order:
///
/// 1. AGENT.md `display_name:` frontmatter — explicit user override.
/// 2. AGENT.md `name:` frontmatter — the technical agent name (which
///    today doubles as the display name when no override is set).
/// 3. `projects.name` from the DB — the workspace folder name as
///    fallback. Empty/missing AGENT.md falls here.
/// 4. The literal "agent" — only if all three above failed (which
///    requires a workspace with no `projects` row, i.e. an unregistered
///    path; should not happen in practice).
pub fn agent_display_name(project_path: &str) -> String {
    let agent_md = workspace_agent_md_path(project_path);
    let mtime = current_mtime(&agent_md);

    {
        let cache = cache().lock().unwrap();
        if let Some(c) = cache.get(project_path) {
            if c.agent_md_mtime == mtime {
                return c.display_name.clone();
            }
        }
    }

    let resolved = resolve_uncached(project_path, &agent_md);

    {
        let mut cache = cache().lock().unwrap();
        cache.insert(
            project_path.to_string(),
            Cached {
                display_name: resolved.clone(),
                agent_md_mtime: mtime,
            },
        );
    }

    resolved
}

fn resolve_uncached(project_path: &str, agent_md: &PathBuf) -> String {
    if let Ok(content) = fs::read_to_string(agent_md) {
        let fm = parse_frontmatter(&content);
        if let Some(d) = fm.get("display_name") {
            if !d.is_empty() {
                return d.clone();
            }
        }
        if let Some(n) = fm.get("name") {
            if !n.is_empty() {
                return n.clone();
            }
        }
    }

    if let Some(name) = lookup_project_name(project_path) {
        if !name.is_empty() {
            return name;
        }
    }

    "agent".to_string()
}

fn lookup_project_name(project_path: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT name FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Validate a candidate display name. Returns `Ok(())` if the input
/// is acceptable, `Err(reason)` otherwise.
///
/// 0.40.24 S3 (Rosson, 2026-07-03): the old slug-shaped rule
/// (lowercase + digits + hyphens only) is deliberately LOOSENED —
/// display names are human labels, and operators want "QA Bot" or
/// "K2 - Marketing Manager". The remaining limits are the ones that
/// protect downstream consumers:
///
/// - non-empty, ≤ 64 chars — the `[from <name>]` PTY prefix and every
///   list/table surface assume a bounded label
///   (`workspace_msg::OWNER_DISPLAY_NAME_MAX` mirrors the 64 ceiling);
/// - no control characters — the name is written into AGENT.md
///   frontmatter as a single `display_name: <value>` line and injected
///   into one-line PTY prefixes (a newline would corrupt both);
/// - no leading/trailing whitespace — names are matched/rendered
///   trimmed everywhere; invisible padding makes addressing flaky;
/// - no `/` — the name doubles as the default archive folder label
///   (`k2 agent retire` → `~/.k2/archive/<name>-<date>/`) and must
///   never smuggle a path separator.
/// - no `:` — federated addresses are `name::host`; a colon in the
///   label would break the user form.
pub fn validate_display_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Display name must not be empty.".to_string());
    }
    if name.chars().count() > 64 {
        return Err("Display name must be at most 64 characters.".to_string());
    }
    if name != name.trim() {
        return Err("Display name must not start or end with whitespace.".to_string());
    }
    if name.contains('/') {
        return Err("Display name must not contain '/'.".to_string());
    }
    if name.contains(':') {
        return Err("Display name must not contain ':' (federated addresses use name::host).".to_string());
    }
    if name.chars().any(char::is_control) {
        return Err("Display name must not contain control characters.".to_string());
    }
    Ok(())
}

/// Write `display_name: <name>` into AGENT.md frontmatter atomically.
/// Creates the field if absent, replaces it if present. Leaves every
/// other frontmatter line and the body untouched.
///
/// If AGENT.md doesn't exist (fresh workspace where the user hasn't
/// gone through Manage Persona yet but is editing the friendly label
/// in Settings), scaffolds a minimal frontmatter with just
/// `display_name:`. The remaining fields (`name:`, `role:`, `type:`)
/// get filled in later when the persona editor / mode setup runs.
/// Reading via `agent_display_name` already tolerates a partial
/// frontmatter, so the stub is enough to make the read path resolve.
///
/// ## Name stores (0.40.24 S3 + federated-handle fix)
///
/// A rename updates **three** surfaces so local addressing, UI labels,
/// and federated `name::host` stamps stay one coherent identity:
///
/// 1. AGENT.md `display_name:` — friendly label (UI / conf).
/// 2. AGENT.md `name:` — technical persona name; federation outbound
///    `from` and roster projection resolve this via
///    [`crate::workspace::agent_identity::resolve_agent_name`]. Keeping
///    it in lockstep with the user-facing rename is what makes
///    Settings / `k2 agent set --name` actually change
///    `[from <name>::<host>]` (previously only `display_name` moved,
///    so peers still saw stale handles like `pod-leader`).
/// 3. `projects.name` — local name-based addressing
///    (`resolve_workspace` / `k2 msg <name>`).
///
/// Canonical PTY map keys remain bare `project_id` (0.37.5) — rewriting
/// `name:` does **not** drop live sessions. Federated user addresses stay
/// the intuitive `<name>::<host>` form (not UUIDs).
///
/// **Uniqueness (host-local):** the lowercased handle must not already
/// belong to another registered workspace (persona `name:` /
/// `display_name:` / `projects.name`). Collisions would make roster
/// resolution AMBIG and break reply routing for `k2 msg name::host`.
///
/// An UNREGISTERED path (no `projects` row yet) still writes AGENT.md
/// only; uniqueness is skipped when there is no peer project list to
/// compare against in practice (still checked when DB is available).
pub fn set_agent_display_name(project_path: &str, name: &str) -> Result<(), String> {
    validate_display_name(name)?;
    if let Some(other) = agent_handle_collision(project_path, name) {
        return Err(format!(
            "Agent name '{name}' is already used by workspace '{other}'. \
             Federated addresses are name::host — names must be unique on this server."
        ));
    }

    let agent_md = workspace_agent_md_path(project_path);

    let content = if agent_md.exists() {
        fs::read_to_string(&agent_md)
            .map_err(|e| format!("Cannot read AGENT.md at {}: {}", agent_md.display(), e))?
    } else {
        if let Some(parent) = agent_md.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create {}: {}", parent.display(), e))?;
        }
        // Empty frontmatter stub. rewrite_frontmatter_field below
        // inserts fields between the fences.
        "---\n---\n\n".to_string()
    };

    // Keep display_name + technical name in lockstep so federation
    // (resolve_agent_name → name:) and UI (display_name) never diverge
    // after a user rename.
    let with_display = rewrite_frontmatter_field(&content, "display_name", name);
    let updated = rewrite_frontmatter_field(&with_display, "name", name);

    crate::workspace::work_item::atomic_write(&agent_md, &updated)?;

    // projects.name (the ADDRESSING name) follows the rename. 0 rows
    // updated = unregistered path — fine; a real DB error is not.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET name = ?1 WHERE path = ?2",
            rusqlite::params![name, project_path],
        )
        .map_err(|e| format!("AGENT.md written, but projects.name update failed: {e}"))?;
    }

    cache().lock().unwrap().remove(project_path);

    Ok(())
}

/// Host-local federated-handle collision check. Returns the **other**
/// workspace's `projects.name` (or path basename) when `candidate` is
/// already taken as a case-insensitive agent handle. Self is excluded
/// by path so re-saving the same name is a no-op success.
fn agent_handle_collision(project_path: &str, candidate: &str) -> Option<String> {
    let want = candidate.trim().to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    let self_path = std::path::Path::new(project_path);
    let projects = crate::projects_ops::projects_list().ok()?;
    for p in projects {
        if std::path::Path::new(&p.path) == self_path {
            continue;
        }
        // Persona technical name (what federation stamps).
        if let Some(agent) = crate::workspace::agent_identity::resolve_agent_name(&p.path) {
            if agent.trim().to_ascii_lowercase() == want {
                return Some(if p.name.trim().is_empty() {
                    p.path.clone()
                } else {
                    p.name.clone()
                });
            }
        }
        // Display override may differ from name: on older trees — still
        // a handle someone might type.
        let display = agent_display_name(&p.path);
        if display.trim().to_ascii_lowercase() == want {
            return Some(if p.name.trim().is_empty() {
                p.path.clone()
            } else {
                p.name.clone()
            });
        }
        // Local addressing name.
        if p.name.trim().to_ascii_lowercase() == want {
            return Some(p.name.clone());
        }
    }
    None
}

/// Replace or insert a single `key: value` line in YAML-ish frontmatter.
/// Preserves field order when the key already exists; appends just
/// before the closing fence when it doesn't. Returns the original
/// content unchanged when no opening fence is present.
///
/// **Safety properties** worth spelling out, since this function is
/// the only thing that ever rewrites a user's persona file:
///
/// 1. Line-anchored fence detection: the opening `---` must be the
///    first line, the closing `---` must be its own line. A `---`
///    substring embedded inside a frontmatter value (e.g.
///    `role: --- the planner ---`) does NOT trigger a false
///    closing-fence match and will not corrupt the file.
/// 2. Body bytes are sliced off whole and re-concatenated verbatim;
///    the body — including any markdown horizontal rules,
///    code fences, fenced examples — is never edited.
/// 3. Comment lines (starting with `#`) are preserved as-is and
///    don't shadow real fields with the same prefix
///    (e.g. `# display_name: do not change` won't be replaced).
/// 4. Original indentation on the matched line is preserved when
///    the field already exists.
/// 5. CRLF line endings are accepted on input; output normalizes to
///    LF (standard for k2so-managed files).
/// 6. Returns the input unchanged on any malformation (no fence pair,
///    not enough lines, etc.) — caller should treat the no-op as the
///    safe failure mode.
fn rewrite_frontmatter_field(content: &str, key: &str, value: &str) -> String {
    // `lines()` strips trailing `\n` and `\r\n` from each line, which
    // gives us a uniform comparison surface for fence detection.
    let lines: Vec<&str> = content.lines().collect();

    // Opening fence must be line 0 (CRLF tolerant).
    if lines.first().map(|l| l.trim_end_matches('\r')) != Some("---") {
        return content.to_string();
    }

    // Closing fence is the FIRST subsequent line that's exactly `---`.
    // Searching by exact-line-match guarantees a `---` substring inside
    // a frontmatter value can't be mistaken for the fence.
    let close_idx = match lines[1..]
        .iter()
        .position(|l| l.trim_end_matches('\r') == "---")
    {
        Some(i) => i + 1,
        None => return content.to_string(),
    };

    let needle_prefix = format!("{key}:");
    let mut found = false;
    let mut new_fm: Vec<String> = Vec::with_capacity(close_idx);

    for line in &lines[1..close_idx] {
        let trimmed = line.trim_start();
        // Comments are preserved verbatim and don't shadow real fields.
        if trimmed.starts_with('#') {
            new_fm.push((*line).to_string());
            continue;
        }
        // Match the first occurrence only (handles malformed files
        // with duplicate keys — first wins, others stay as-is).
        if !found && trimmed.starts_with(&needle_prefix) {
            found = true;
            let indent_len = line.len() - line.trim_start().len();
            let indent = &line[..indent_len];
            new_fm.push(format!("{indent}{key}: {value}"));
        } else {
            new_fm.push((*line).to_string());
        }
    }
    if !found {
        new_fm.push(format!("{key}: {value}"));
    }

    // Reassemble. Output is LF-normalized regardless of input EOL
    // style. Trailing newline preservation matches the original
    // content's last byte so we don't add or remove the file's
    // final blank line.
    let trailing_newline = content.ends_with('\n');
    let body_lines = &lines[close_idx..];

    let mut out = String::with_capacity(content.len() + value.len() + key.len() + 8);
    out.push_str("---\n");
    for line in &new_fm {
        out.push_str(line);
        out.push('\n');
    }
    for (i, line) in body_lines.iter().enumerate() {
        out.push_str(line);
        // Insert a newline between every body line, plus one at the
        // end if the original had one.
        if i + 1 < body_lines.len() || trailing_newline {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Validator contract (0.40.24 S3 — loosened deliberately) ────
    //
    // Display names are HUMAN labels: spaces and mixed case are first-
    // class ("QA Bot", "K2 - Marketing Manager"). What stays banned is
    // exactly what breaks downstream consumers: empty, >64 chars,
    // control chars (one-line frontmatter + PTY prefix), surrounding
    // whitespace (trimmed matching everywhere), and "/" (the name
    // seeds retire's archive folder label).

    #[test]
    fn validate_accepts_simple_name() {
        assert!(validate_display_name("scout").is_ok());
        assert!(validate_display_name("scout-2").is_ok());
        assert!(validate_display_name("ab").is_ok());
        // Single-character names are legal — "non-empty" is the floor.
        assert!(validate_display_name("a").is_ok());
        // Exactly at the 64-char ceiling.
        assert!(validate_display_name(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_accepts_spaces_and_mixed_case() {
        assert!(validate_display_name("Scout").is_ok());
        assert!(validate_display_name("QA Bot").is_ok());
        assert!(validate_display_name("K2 - Marketing Manager").is_ok());
        // Internal punctuation and non-ASCII are label content, not a
        // problem for any consumer — allowed.
        assert!(validate_display_name("Björn (Ops #2)").is_ok());
        // Leading hyphen / trailing hyphen were slug-era rules; a human
        // label may carry them.
        assert!(validate_display_name("-leading").is_ok());
        assert!(validate_display_name("trailing-").is_ok());
    }

    #[test]
    fn validate_rejects_bad_shape() {
        // Empty.
        assert!(validate_display_name("").is_err());
        // Over the 64-char ceiling (chars, not bytes).
        assert!(validate_display_name(&"x".repeat(65)).is_err());
        // Leading/trailing whitespace (any kind).
        assert!(validate_display_name(" padded").is_err());
        assert!(validate_display_name("padded ").is_err());
        assert!(validate_display_name("\tQA Bot").is_err());
        // A whitespace-only name is both "empty after trim" and
        // padded — rejected either way.
        assert!(validate_display_name("   ").is_err());
        // Control characters (would corrupt the one-line frontmatter
        // field and the `[from <name>]` PTY prefix).
        assert!(validate_display_name("QA\nBot").is_err());
        assert!(validate_display_name("QA\x1bBot").is_err());
        assert!(validate_display_name("QA\tBot").is_err());
        // Path separator (archive-folder label safety).
        assert!(validate_display_name("ops/qa").is_err());
        // Colon (federated name::host safety).
        assert!(validate_display_name("ops:qa").is_err());
        assert!(validate_display_name("a::b").is_err());
    }

    #[test]
    fn set_display_name_writes_both_name_stores() {
        // Rosson's 0.40.24 S3 decision: a rename must land in BOTH
        // stores — AGENT.md `display_name:` (the label) AND
        // `projects.name` (the addressing name resolve_workspace
        // matches). Uses the shared in-memory test DB + a real temp
        // dir for the AGENT.md write.
        let dir = std::env::temp_dir().join(format!(
            "k2-display-bothstores-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create workspace dir");
        let path = dir.to_string_lossy().to_string();
        let project_id = uuid::Uuid::new_v4().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![project_id, "old-name", path],
            )
            .expect("insert project row");
        }

        set_agent_display_name(&path, "QA Bot").expect("rename must succeed");

        // Store 1: AGENT.md frontmatter (and the read path resolves it,
        // spaces intact).
        assert_eq!(agent_display_name(&path), "QA Bot");
        // Technical name (federation / resolve_agent_name) stays in lockstep.
        let md = std::fs::read_to_string(workspace_agent_md_path(&path)).expect("AGENT.md");
        assert!(
            md.lines().any(|l| l.trim() == "name: QA Bot"),
            "technical name: must match rename so federation stamps name::host correctly; got:\n{md}"
        );
        assert!(
            md.lines().any(|l| l.trim() == "display_name: QA Bot"),
            "display_name: must match rename; got:\n{md}"
        );
        // Store 2: projects.name (addressing).
        let db_name: String = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT name FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |r| r.get(0),
            )
            .expect("project row still present")
        };
        assert_eq!(db_name, "QA Bot", "projects.name must follow the rename");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_display_name_rejects_host_local_handle_collision() {
        let suffix = uuid::Uuid::new_v4();
        let dir_a = std::env::temp_dir().join(format!(
            "k2-display-collide-a-{}-{}",
            std::process::id(),
            suffix
        ));
        let dir_b = std::env::temp_dir().join(format!(
            "k2-display-collide-b-{}-{}",
            std::process::id(),
            suffix
        ));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let path_a = dir_a.to_string_lossy().to_string();
        let path_b = dir_b.to_string_lossy().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), "alpha", path_a],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), "beta", path_b],
            )
            .unwrap();
        }
        set_agent_display_name(&path_a, "Scout").expect("first claim");
        let err = set_agent_display_name(&path_b, "scout")
            .expect_err("case-insensitive collision must reject");
        assert!(
            err.to_ascii_lowercase().contains("already used"),
            "expected uniqueness error, got: {err}"
        );
        // Same workspace re-save is fine.
        set_agent_display_name(&path_a, "Scout").expect("idempotent self rename");

        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn set_display_name_on_unregistered_path_still_writes_agent_md() {
        // No projects row: the AGENT.md label write must still succeed
        // (0 DB rows updated is NOT an error — only a real DB failure is).
        let dir = std::env::temp_dir().join(format!(
            "k2-display-unregistered-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create workspace dir");
        let path = dir.to_string_lossy().to_string();

        set_agent_display_name(&path, "Solo Label").expect("unregistered rename ok");
        assert_eq!(agent_display_name(&path), "Solo Label");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rewrite_replaces_existing_field() {
        let md = "---\nname: scout\ndisplay_name: old\ntype: custom\n---\nbody\n";
        let out = rewrite_frontmatter_field(md, "display_name", "new");
        assert!(out.contains("display_name: new"));
        assert!(!out.contains("display_name: old"));
        assert!(out.contains("name: scout"));
        assert!(out.contains("type: custom"));
        assert!(out.contains("body"));
    }

    #[test]
    fn rewrite_inserts_when_missing() {
        let md = "---\nname: scout\ntype: custom\n---\nbody\n";
        let out = rewrite_frontmatter_field(md, "display_name", "new");
        assert!(out.contains("display_name: new"));
        assert!(out.contains("name: scout"));
        assert!(out.contains("type: custom"));
        assert!(out.contains("body"));
        // The closing fence must remain.
        assert!(out.matches("---").count() >= 2);
    }

    #[test]
    fn rewrite_noops_without_frontmatter() {
        let md = "no frontmatter here\n";
        let out = rewrite_frontmatter_field(md, "display_name", "x");
        assert_eq!(out, md);
    }

    #[test]
    fn rewrite_scaffold_into_empty_frontmatter() {
        // Mirrors the path set_agent_display_name takes on a fresh
        // workspace: empty AGENT.md stub → first display_name write.
        let md = "---\n---\n\n";
        let out = rewrite_frontmatter_field(md, "display_name", "scout");
        assert!(out.contains("display_name: scout"));
        assert_eq!(out.matches("---").count(), 2);
    }

    #[test]
    fn rewrite_does_not_break_on_dashes_in_value() {
        // Adversarial: a frontmatter value contains the `---`
        // substring. The naive impl would mistake this for the closing
        // fence and corrupt the file. Line-anchored detection must
        // skip past it and find the real closing fence.
        let md = "---\nname: scout\nrole: --- the planner ---\ntype: custom\n---\n\nbody here\n";
        let out = rewrite_frontmatter_field(md, "display_name", "ranger");
        assert!(out.contains("display_name: ranger"));
        assert!(out.contains("role: --- the planner ---"));
        assert!(out.contains("name: scout"));
        assert!(out.contains("type: custom"));
        assert!(out.contains("body here"));
        assert_eq!(out.matches("---").count(), 4); // open, value(2x ---), close
    }

    #[test]
    fn rewrite_preserves_body_with_horizontal_rule() {
        // Body contains markdown `---` (a horizontal rule). Must not
        // affect frontmatter parsing.
        let md = "---\nname: scout\n---\n\n# heading\n\n---\n\nbelow rule\n";
        let out = rewrite_frontmatter_field(md, "display_name", "ranger");
        assert!(out.contains("display_name: ranger"));
        assert!(out.contains("# heading"));
        assert!(out.contains("below rule"));
        // The body's `---` rule must still be there.
        let body_start = out.find("\n\n# heading").unwrap();
        let body = &out[body_start..];
        assert!(body.contains("\n---\n"));
    }

    #[test]
    fn rewrite_preserves_large_body_byte_for_byte() {
        // Lots of stuff in the body — code fences, lists, quotes.
        // Frontmatter rewrite should leave it byte-identical.
        let body = "\n# Persona\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n- item 1\n- item 2\n\n> a quote\n\nMore prose here.\n";
        let md = format!("---\nname: scout\n---{body}");
        let out = rewrite_frontmatter_field(&md, "display_name", "ranger");
        // Find the second `---` and assert everything after it
        // matches the original body verbatim.
        let second_fence = out.match_indices("---").nth(1).unwrap().0;
        let written_body = &out[second_fence + 3..];
        assert_eq!(written_body, body, "body must be byte-identical");
    }

    #[test]
    fn rewrite_skips_comment_with_same_prefix() {
        // A comment line that LOOKS like a `display_name:` field must
        // not be replaced. The real field gets added below.
        let md = "---\nname: scout\n# display_name: do not change me\n---\n\nbody\n";
        let out = rewrite_frontmatter_field(md, "display_name", "ranger");
        assert!(out.contains("# display_name: do not change me"));
        assert!(out.contains("display_name: ranger"));
    }

    #[test]
    fn rewrite_preserves_indentation_on_replace() {
        let md = "---\nname: scout\n  display_name: old\n---\nbody\n";
        let out = rewrite_frontmatter_field(md, "display_name", "new");
        assert!(out.contains("  display_name: new"));
        assert!(!out.contains("display_name: old"));
    }

    #[test]
    fn rewrite_handles_crlf_input() {
        let md = "---\r\nname: scout\r\n---\r\n\r\nbody\r\n";
        let out = rewrite_frontmatter_field(md, "display_name", "ranger");
        // Output is LF-normalized — that's documented behavior.
        assert!(out.contains("display_name: ranger"));
        assert!(out.contains("name: scout"));
        assert!(out.contains("body"));
    }

    #[test]
    fn rewrite_noops_on_unterminated_frontmatter() {
        // Opening fence but no closing one — return content unchanged.
        let md = "---\nname: scout\nno closing fence here\n";
        let out = rewrite_frontmatter_field(md, "display_name", "x");
        assert_eq!(out, md);
    }
}
