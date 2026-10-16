//! Changes tab v1.1 — review notes (C5, `.k2/prds/prd-changes-tab-v1.md`
//! "C5"). A person leaves a note on a line or a range of lines of a changed
//! file; "Send to agent" batches the open unsent notes into one message to
//! the workspace's agent (the daemon delivers it through the `k2 msg`
//! path); the agent resolves each note when it is fixed.
//!
//! This module is the store (table `git_review_notes`, migration 0178), the
//! anchor logic (follow a moved line, flag a rewritten one as outdated) and
//! the message format. The routes live in `k2-daemon/src/git_notes_routes.rs`.
//!
//! The message format follows the idea of Orca's `diff-comments-format.ts`
//! (`File / Line(s) / comment` blocks, MIT, Lovecast Inc. 2026); the code
//! here is K2's own, nothing is copied.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

/// A note body longer than this is refused.
pub const MAX_BODY_CHARS: usize = 20_000;
/// The widest range one note may cover.
pub const MAX_LINE_SPAN: i64 = 5_000;
/// Open (unresolved) notes per workspace path.
pub const MAX_OPEN_PER_WORKSPACE: i64 = 1_000;
/// One sent message stays under this many chars; the rest wait for the
/// next send.
pub const MAX_MESSAGE_CHARS: usize = 60_000;

/// One review note (the route's wire shape: camelCase).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewNote {
    pub id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    /// Repo-relative file path, `/` separators.
    pub path: String,
    /// `new` (the changed text) or `old` (the text before the change).
    pub side: String,
    pub start_line: i64,
    pub end_line: i64,
    pub body: String,
    pub author: String,
    pub author_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub sent_at: Option<i64>,
    pub delivery_id: Option<String>,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
    pub rev: Option<String>,
    pub blob: Option<String>,
    pub anchor_text: Option<String>,
}

impl ReviewNote {
    pub fn is_open(&self) -> bool {
        self.resolved_at.is_none()
    }
    pub fn is_unsent(&self) -> bool {
        self.resolved_at.is_none() && self.sent_at.is_none()
    }
}

/// List filter (`GET /cli/git/notes?status=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not resolved (sent or not). The default.
    Open,
    /// Not resolved and not sent yet.
    Unsent,
    /// Not resolved, already sent to the agent.
    Sent,
    Resolved,
    All,
}

impl Status {
    pub fn parse(s: Option<&str>) -> Option<Status> {
        match s.map(str::trim).unwrap_or("") {
            "" | "open" => Some(Status::Open),
            "unsent" => Some(Status::Unsent),
            "sent" => Some(Status::Sent),
            "resolved" => Some(Status::Resolved),
            "all" => Some(Status::All),
            _ => None,
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Status::Open => "resolved_at IS NULL",
            Status::Unsent => "resolved_at IS NULL AND sent_at IS NULL",
            Status::Sent => "resolved_at IS NULL AND sent_at IS NOT NULL",
            Status::Resolved => "resolved_at IS NOT NULL",
            Status::All => "1 = 1",
        }
    }
}

/// Per-workspace totals for the Changes pane's Notes header.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteCounts {
    pub open: i64,
    pub unsent: i64,
    pub sent: i64,
    pub resolved: i64,
}

const COLS: &str = "id, workspace_id, workspace_path, path, side, start_line, end_line, body, author, \
    author_name, created_at, updated_at, sent_at, delivery_id, resolved_at, resolved_by, rev, blob, anchor_text";

fn from_row(r: &Row<'_>) -> rusqlite::Result<ReviewNote> {
    Ok(ReviewNote {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        workspace_path: r.get(2)?,
        path: r.get(3)?,
        side: r.get(4)?,
        start_line: r.get(5)?,
        end_line: r.get(6)?,
        body: r.get(7)?,
        author: r.get(8)?,
        author_name: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        sent_at: r.get(12)?,
        delivery_id: r.get(13)?,
        resolved_at: r.get(14)?,
        resolved_by: r.get(15)?,
        rev: r.get(16)?,
        blob: r.get(17)?,
        anchor_text: r.get(18)?,
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `n_` + 12 hex chars: short enough to type in `k2 changes notes resolve`.
pub fn new_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("n_{}", &u[..12])
}

/// Store-side cleanup of a note body: no control characters except
/// newline and tab (a body is typed into the agent's terminal later), CRLF
/// folded to LF, surrounding blank space trimmed.
pub fn clean_body(body: &str) -> String {
    let unified = body.replace("\r\n", "\n");
    let kept: String = unified.chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect();
    kept.trim().to_string()
}

pub fn insert(conn: &Connection, n: &ReviewNote) -> rusqlite::Result<()> {
    conn.execute(
        &format!("INSERT INTO git_review_notes ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"),
        params![
            n.id,
            n.workspace_id,
            n.workspace_path,
            n.path,
            n.side,
            n.start_line,
            n.end_line,
            n.body,
            n.author,
            n.author_name,
            n.created_at,
            n.updated_at,
            n.sent_at,
            n.delivery_id,
            n.resolved_at,
            n.resolved_by,
            n.rev,
            n.blob,
            n.anchor_text,
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<ReviewNote>> {
    conn.query_row(&format!("SELECT {COLS} FROM git_review_notes WHERE id = ?1"), params![id], from_row)
        .optional()
}

/// Notes on one workspace path, by file then line then age.
pub fn list(conn: &Connection, workspace_path: &str, status: Status, file: Option<&str>) -> rusqlite::Result<Vec<ReviewNote>> {
    let sql = format!(
        "SELECT {COLS} FROM git_review_notes WHERE workspace_path = ?1 AND {} AND (?2 IS NULL OR path = ?2) \
         ORDER BY path, start_line, created_at, id",
        status.sql()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![workspace_path, file], from_row)?;
    rows.collect()
}

pub fn counts(conn: &Connection, workspace_path: &str) -> rusqlite::Result<NoteCounts> {
    conn.query_row(
        "SELECT \
           COALESCE(SUM(resolved_at IS NULL), 0), \
           COALESCE(SUM(resolved_at IS NULL AND sent_at IS NULL), 0), \
           COALESCE(SUM(resolved_at IS NULL AND sent_at IS NOT NULL), 0), \
           COALESCE(SUM(resolved_at IS NOT NULL), 0) \
         FROM git_review_notes WHERE workspace_path = ?1",
        params![workspace_path],
        |r| Ok(NoteCounts { open: r.get(0)?, unsent: r.get(1)?, sent: r.get(2)?, resolved: r.get(3)? }),
    )
}

pub fn update_body(conn: &Connection, id: &str, body: &str, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE git_review_notes SET body = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, body, now],
    )?;
    Ok(n == 1)
}

/// Resolve (`by` = who) or reopen (`by` = None) one note. Reopening also
/// clears `sent_at`, so the note goes out again with the next send.
pub fn set_resolved(conn: &Connection, id: &str, by: Option<&str>, now: i64) -> rusqlite::Result<bool> {
    let n = match by {
        Some(who) => conn.execute(
            "UPDATE git_review_notes SET resolved_at = ?2, resolved_by = ?3, updated_at = ?2 WHERE id = ?1",
            params![id, now, who],
        )?,
        None => conn.execute(
            "UPDATE git_review_notes SET resolved_at = NULL, resolved_by = NULL, sent_at = NULL, \
             delivery_id = NULL, updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?,
    };
    Ok(n == 1)
}

pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM git_review_notes WHERE id = ?1", params![id])? == 1)
}

/// Mark notes sent, once: only rows still open and unsent change. Returns
/// how many did.
pub fn mark_sent(conn: &Connection, ids: &[String], delivery_id: Option<&str>, now: i64) -> rusqlite::Result<usize> {
    let mut n = 0;
    for id in ids {
        n += conn.execute(
            "UPDATE git_review_notes SET sent_at = ?2, delivery_id = ?3, updated_at = ?2 \
             WHERE id = ?1 AND sent_at IS NULL AND resolved_at IS NULL",
            params![id, now, delivery_id],
        )?;
    }
    Ok(n)
}

// ── Anchors ──────────────────────────────────────────────────────────

/// A text's lines as the editor numbers them: no trailing empty line for a
/// final newline, `\r` dropped from CRLF endings.
pub fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// The text of lines `start..=end` (1-based) joined with `\n`, or None when
/// the range is outside the text.
pub fn anchor_from_text(text: &str, start: i64, end: i64) -> Option<String> {
    let lines = split_lines(text);
    if start < 1 || end < start || end as usize > lines.len() {
        return None;
    }
    Some(lines[(start - 1) as usize..end as usize].join("\n"))
}

/// Where a note's lines are in `text` now: its own range when the text
/// there still matches the anchor, else the nearest place the same lines
/// moved to, else None (outdated). A note with no anchor stays put while
/// its range still exists.
pub fn locate(anchor: Option<&str>, start: i64, end: i64, text: &str) -> Option<(i64, i64)> {
    let lines = split_lines(text);
    let Some(anchor) = anchor else {
        return (start >= 1 && end >= start && (end as usize) <= lines.len()).then_some((start, end));
    };
    let want: Vec<&str> = anchor.split('\n').collect();
    let span = want.len();
    if span == 0 || lines.len() < span {
        return None;
    }
    let at = |i: usize| lines[i..i + span] == want[..];
    let orig = (start - 1).max(0) as usize;
    if orig + span <= lines.len() && at(orig) {
        return Some((start, start + span as i64 - 1));
    }
    let mut best: Option<usize> = None;
    for i in 0..=lines.len() - span {
        if !at(i) {
            continue;
        }
        let closer = match best {
            None => true,
            Some(b) => i.abs_diff(orig) < b.abs_diff(orig),
        };
        if closer {
            best = Some(i);
        }
    }
    best.map(|i| (i as i64 + 1, (i + span) as i64))
}

/// The texts a note's side may be shown against today, best first:
/// `new` = the working-tree file, then the index copy (a staged row's new
/// side); `old` = HEAD, then the index (an unstaged row's old side). Only
/// UTF-8 text up to the Changes caps; missing, binary or huge sides are
/// left out. `rel` is repo-relative.
pub fn side_texts(repo_path: &str, rel: &str, side: &str) -> Vec<String> {
    use crate::git_changes::{looks_binary, MAX_BLOB_BYTES};
    let Ok(repo) = git2::Repository::discover(repo_path) else {
        return Vec::new();
    };
    let text_of = |bytes: &[u8]| -> Option<String> {
        if bytes.len() as u64 > MAX_BLOB_BYTES || looks_binary(bytes) {
            return None;
        }
        String::from_utf8(bytes.to_vec()).ok()
    };
    let index_text = || -> Option<String> {
        let idx = repo.index().ok()?;
        let entry = idx.get_path(std::path::Path::new(rel), 0)?;
        let blob = repo.find_blob(entry.id).ok()?;
        text_of(blob.content())
    };
    let head_text = || -> Option<String> {
        let tree = repo.head().ok()?.peel_to_tree().ok()?;
        let entry = tree.get_path(std::path::Path::new(rel)).ok()?;
        let blob = repo.find_blob(entry.id()).ok()?;
        text_of(blob.content())
    };
    let work_text = || -> Option<String> {
        let full = repo.workdir()?.join(rel);
        let meta = std::fs::symlink_metadata(&full).ok()?;
        if !meta.is_file() || meta.len() > MAX_BLOB_BYTES {
            return None;
        }
        text_of(&std::fs::read(&full).ok()?)
    };
    let mut out: Vec<String> = Vec::new();
    let candidates = if side == "old" { [head_text(), index_text()] } else { [work_text(), index_text()] };
    for t in candidates.into_iter().flatten() {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The git blob id of a side's text as it is now (`new` = the working-tree
/// file, `old` = HEAD's copy), or None.
pub fn side_blob_id(repo_path: &str, rel: &str, side: &str) -> Option<String> {
    let repo = git2::Repository::discover(repo_path).ok()?;
    if side == "old" {
        let tree = repo.head().ok()?.peel_to_tree().ok()?;
        return tree.get_path(std::path::Path::new(rel)).ok().map(|e| e.id().to_string());
    }
    let full = repo.workdir()?.join(rel);
    if !full.is_file() {
        return None;
    }
    git2::Oid::hash_file(git2::ObjectType::Blob, &full).ok().map(|o| o.to_string())
}

/// Where a note is now against the texts its side may show: the first
/// text it is found in (moved or not) wins; outdated when none.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotePlace {
    pub outdated: bool,
    pub current_start_line: Option<i64>,
    pub current_end_line: Option<i64>,
}

pub fn place(note: &ReviewNote, texts: &[String]) -> NotePlace {
    for t in texts {
        if let Some((s, e)) = locate(note.anchor_text.as_deref(), note.start_line, note.end_line, t) {
            return NotePlace { outdated: false, current_start_line: Some(s), current_end_line: Some(e) };
        }
    }
    NotePlace { outdated: true, current_start_line: None, current_end_line: None }
}

// ── The message an agent gets ─────────────────────────────────────────

fn line_label(n: &ReviewNote) -> String {
    let lines = if n.start_line == n.end_line {
        format!("Line: {}", n.start_line)
    } else {
        format!("Lines: {}-{}", n.start_line, n.end_line)
    };
    if n.side == "old" {
        format!("{lines} (in the text before the change)")
    } else {
        lines
    }
}

fn block(n: &ReviewNote) -> String {
    let mut body = n.body.lines();
    let first = body.next().unwrap_or("");
    let mut out = format!("File: {}\n{}\nComment: {first}", n.path, line_label(n));
    for l in body {
        out.push_str("\n  ");
        out.push_str(l);
    }
    out.push_str(&format!("\nNote id: {}", n.id));
    out
}

fn header(count: usize, workspace_path: &str) -> String {
    format!(
        "Review notes on your changes in {workspace_path} ({count} note{}). Please address each one.",
        if count == 1 { "" } else { "s" }
    )
}

const FOOTER: &str = "When a note is fixed, resolve it: `k2 changes notes resolve <note id>`. \
List them any time with `k2 changes notes`.";

/// Build the one message for `notes` (in order), taking as many as fit in
/// [`MAX_MESSAGE_CHARS`] (always at least one). Returns the text and how
/// many notes it carries.
pub fn format_message(notes: &[ReviewNote], workspace_path: &str) -> (String, usize) {
    if notes.is_empty() {
        return (String::new(), 0);
    }
    let blocks: Vec<String> = notes.iter().map(block).collect();
    let fixed = header(notes.len(), workspace_path).len() + FOOTER.len() + 8;
    let mut take = 0;
    let mut used = fixed;
    for b in &blocks {
        if take > 0 && used + b.len() + 2 > MAX_MESSAGE_CHARS {
            break;
        }
        used += b.len() + 2;
        take += 1;
    }
    let mut out = header(take, workspace_path);
    for b in &blocks[..take] {
        out.push_str("\n\n");
        out.push_str(b);
    }
    out.push_str("\n\n");
    out.push_str(FOOTER);
    (out, take)
}

#[cfg(test)]
#[path = "git_notes_tests.rs"]
mod tests;
