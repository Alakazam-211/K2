//! Garden and widget edits through the daemon (agents run Gardens end to
//! end, 0.46.0, migration 0181).
//!
//! Two writers, both checked before anything touches the disk:
//! - [`ZenFiles::put_garden`] writes one user Garden page,
//!   `gardens/<id>.toml`. The Garden must exist and not be locked; the
//!   text must check clean against the Garden's own defaults (its sync
//!   state), so a bad page is refused, not saved. The previous text is
//!   kept as a snapshot (`k2 zen reset --to` undoes it). Sync state is
//!   never touched: editing a file never flips it (GS, never-revert).
//! - [`ZenFiles::put_widget_file`] writes one code file (`.html`, `.js`,
//!   `.css`, `.json`) in a user widget folder, `widgets/<name>/<file>`.
//!   Built-in widgets (`k2:<name>`) are K2's frozen defaults and can't be
//!   written; copy one with `k2 zen widget new --from`.
//!
//! Nothing here writes zen.toml, themes, `.defaults/`, `.locks/`,
//! `.history/` (except the undo snapshot), `gardens.json`, `sync.json` or
//! any template: those are K2's or the person's.
//!
//! The proposal queue (`zen_proposals`): an agent without "Manage Gardens
//! and data sources" proposes the same change; a person (or an agent with
//! the switch) approves it, which applies it only if the file is still the
//! one the proposal was made against (`base_hash`).
//!
//! A folder put (`k2 zen widget put <name> --dir`) is ONE proposal for the
//! whole folder (0.46.3, "Remaining Garden gaps"): approving it writes every
//! changed file at once, or nothing. No migration: 0181's `kind` CHECK
//! allows only 'garden' and 'widget', so a folder proposal is stored as a
//! widget row whose `file` is [`FOLDER_FILE`] (`*`, never a valid file
//! name) and whose `content` is the [`super::widget_folder::FolderProposal`]
//! JSON. The API names its kind `folder` and gives it no `file`.

use std::fs;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use super::store::{ZenError, ZenFile, ZenFiles};
use super::widgets::{self, CODE_EXTS};

/// A widget code file is at most this many bytes (the bundle's code cap).
pub const MAX_PUT_BYTES: usize = super::bundle::MAX_CODE_BYTES;
/// A Garden page is at most this many bytes.
pub const MAX_GARDEN_BYTES: usize = 256 * 1024;
/// A proposal note is at most this many characters.
pub const MAX_NOTE_CHARS: usize = 500;
/// The `file` column of a folder proposal's row (see the module doc).
pub const FOLDER_FILE: &str = "*";

/// `sha256` hex of `text`, or `""` for a file that doesn't exist.
pub fn text_hash(text: Option<&str>) -> String {
    bytes_hash(text.map(str::as_bytes))
}

/// `sha256` hex of `bytes`, or `""` for a file that doesn't exist (the same
/// value as [`text_hash`] for text).
pub fn bytes_hash(bytes: Option<&[u8]>) -> String {
    match bytes {
        None => String::new(),
        Some(b) => Sha256::digest(b).iter().map(|b| format!("{b:02x}")).collect(),
    }
}

/// A widget code file name: one path segment, lower-case letters, digits,
/// `-`, `_` and `.`, with a code extension. Never a dot file, never `..`.
pub fn valid_widget_file(name: &str) -> bool {
    let ok_chars = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && !name.contains("..");
    ok_chars && name.rsplit_once('.').is_some_and(|(_, ext)| CODE_EXTS.contains(&ext))
}

impl ZenFiles {
    /// The current text of a user Garden page (`None`: no file yet) and its
    /// id. Unknown Garden → 404.
    pub fn garden_text(&self, garden: &str) -> Result<(String, Option<String>), ZenError> {
        self.require()?;
        let (_, g) = self.garden(garden)?;
        let text = match self.read_disk(&ZenFile::Garden(g.id.clone())) {
            None => None,
            Some(Ok(t)) => Some(t),
            Some(Err(m)) => return Err(ZenError::Io(m)),
        };
        Ok((g.id, text))
    }

    /// The current text of a widget code file (`None`: no file yet) and
    /// the folder name. A built-in widget or a bad file name → 400.
    pub fn widget_file_text(&self, widget: &str, file: &str) -> Result<(String, Option<String>), ZenError> {
        self.require()?;
        let name = self.user_widget_name(widget)?;
        if !valid_widget_file(file) {
            return Err(ZenError::BadRequest(format!(
                "'{file}' is not a widget code file: one name ending .html, .js, .css or .json (lower-case letters, digits, - _ .)"
            )));
        }
        let p = self.widget_dir(&name).join(file);
        let text = match fs::read(&p) {
            Ok(b) => Some(String::from_utf8(b).map_err(|_| ZenError::BadRequest(format!("{file} is not UTF-8 text")))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(ZenError::Io(format!("{}: {e}", p.display()))),
        };
        Ok((name, text))
    }

    /// Check a Garden page's text without writing it: `Err(BadRequest)`
    /// listing the errors (at most 10) when it doesn't check clean, and
    /// 423 `garden_locked` for a locked Garden unless the person writes it
    /// (`by_person`: the lock then follows the new text, like reset).
    pub fn check_garden_put(&self, garden: &str, text: &str, by_person: bool) -> Result<String, ZenError> {
        self.require()?;
        let (_, g) = self.garden(garden)?;
        if !by_person {
            self.refuse_if_locked(&g.id)?;
        }
        if text.len() > MAX_GARDEN_BYTES {
            return Err(ZenError::BadRequest(format!("a Garden page is at most {} KiB", MAX_GARDEN_BYTES / 1024)));
        }
        let f = ZenFile::Garden(g.id.clone());
        let base = self.base_for(&f);
        let checked = self.check_src(&f, text, &base);
        if !checked.errors.is_empty() {
            let lines: Vec<String> = checked.errors.iter().take(10).map(|d| d.render()).collect();
            return Err(ZenError::BadRequest(format!("the page has errors, nothing was written: {}", lines.join(" | "))));
        }
        Ok(g.id)
    }

    /// Write one user Garden page (`gardens/<id>.toml`) after
    /// [`ZenFiles::check_garden_put`]. The old text is kept as a snapshot.
    pub fn put_garden(&self, garden: &str, text: &str, by_person: bool) -> Result<J, ZenError> {
        let id = self.check_garden_put(garden, text, by_person)?;
        let f = ZenFile::Garden(id.clone());
        let path = self.path_of(&f);
        let mut kept = None;
        if let Some(Ok(current)) = self.read_disk(&f) {
            if current == text {
                return Ok(json!({ "ok": true, "garden": id, "file": f.label(), "changed": false, "hash": text_hash(Some(text)) }));
            }
            let newest = self.snapshots(&f).into_iter().next();
            kept = match newest {
                Some(s) if self.snapshot_text(&f, &s.name).as_deref() == Some(current.as_str()) => Some(s.name),
                _ => Some(self.write_snapshot(&f, &current)?),
            };
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ZenError::Io(format!("{}: {e}", parent.display())))?;
        }
        crate::fs_atomic::atomic_write_str(&path, text).map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))?;
        self.prune(&f);
        if by_person {
            // The person may change a locked Garden: the lock holds the new
            // text (agents never reach here for a locked one).
            self.lock_follow_owner_write(&id)?;
        }
        Ok(json!({
            "ok": true,
            "garden": id,
            "file": f.label(),
            "changed": true,
            "snapshot": kept,
            "hash": text_hash(Some(text)),
        }))
    }

    /// Check a widget file write without writing it: the folder name.
    pub fn check_widget_put(&self, widget: &str, file: &str, text: &str) -> Result<String, ZenError> {
        let (name, _) = self.widget_file_text(widget, file)?;
        if text.len() > MAX_PUT_BYTES {
            return Err(ZenError::BadRequest(format!(
                "a widget code file is at most {} ({MAX_PUT_BYTES} bytes); this one is {} bytes. Split the code, or use K2's libraries (k2 zen guide libs)",
                super::bundle::human_bytes(MAX_PUT_BYTES),
                text.len()
            )));
        }
        if file == widgets::MANIFEST_FILE && serde_json::from_str::<J>(text).is_err() {
            return Err(ZenError::BadRequest("manifest.json is not JSON; nothing was written".into()));
        }
        Ok(name)
    }

    /// Write one code file in a user widget folder (made if missing), then
    /// check the folder. The answer carries `k2 zen validate --widget`'s
    /// result: a file that leaves the widget with errors is still written
    /// (the last good version stays live), and the errors say what to fix.
    pub fn put_widget_file(&self, widget: &str, file: &str, text: &str) -> Result<J, ZenError> {
        let name = self.check_widget_put(widget, file, text)?;
        let dir = self.widget_dir(&name);
        fs::create_dir_all(&dir).map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?;
        let p = dir.join(file);
        crate::fs_atomic::atomic_write_str(&p, text).map_err(|e| ZenError::Io(format!("{}: {e}", p.display())))?;
        let validate = self.validate_widget(&name).unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }));
        Ok(json!({
            "ok": true,
            "widget": name,
            "file": format!("{}/{name}/{file}", widgets::WIDGETS_DIR),
            "changed": true,
            "hash": text_hash(Some(text)),
            "validate": validate,
        }))
    }
}

// ── the proposal queue (zen_proposals, 0181) ──────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalKind {
    Garden,
    Widget,
    /// A whole widget folder (stored as a widget row, file [`FOLDER_FILE`]).
    Folder,
}

impl ProposalKind {
    /// The row's `kind` column.
    pub fn code(self) -> &'static str {
        match self {
            ProposalKind::Garden => "garden",
            ProposalKind::Widget | ProposalKind::Folder => "widget",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub kind: ProposalKind,
    /// A Garden id, or a widget folder name.
    pub target: String,
    /// The widget file; `None` for a Garden page.
    pub file: Option<String>,
    #[serde(skip_serializing)]
    pub content: String,
    pub base_hash: String,
    pub note: Option<String>,
    pub status: String,
    pub made_by: String,
    pub made_by_project: String,
    pub made_by_session: Option<String>,
    pub created_at: i64,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
}

impl Proposal {
    /// The list view: everything but the content, plus its size.
    pub fn summary(&self) -> J {
        let mut v = serde_json::to_value(self).unwrap_or(J::Null);
        v["bytes"] = json!(self.content.len());
        if self.kind == ProposalKind::Folder {
            // A folder proposal also names the files it changes and how
            // many staged files it keeps as they are.
            match self.folder() {
                Ok(f) => {
                    v["files"] = json!(f.files.iter().map(|x| x.name.as_str()).collect::<Vec<_>>());
                    v["unchanged"] = json!(f.keep.len());
                }
                Err(e) => v["error"] = json!(e),
            }
        }
        v
    }

    /// A folder proposal's files. `Err` for any other kind, or content that
    /// doesn't parse (K2 never writes such a row).
    pub fn folder(&self) -> Result<super::widget_folder::FolderProposal, String> {
        if self.kind != ProposalKind::Folder {
            return Err(format!("{} is not a folder proposal", self.id));
        }
        serde_json::from_str(&self.content).map_err(|e| format!("{}: the folder proposal doesn't parse: {e}", self.id))
    }
}

/// A new proposal.
pub struct NewProposal<'a> {
    pub kind: ProposalKind,
    pub target: &'a str,
    pub file: Option<&'a str>,
    pub content: &'a str,
    pub base_hash: &'a str,
    pub note: Option<&'a str>,
    pub made_by: &'a str,
    pub made_by_project: &'a str,
    pub made_by_session: Option<&'a str>,
    pub now: i64,
}

const COLS: &str = "id, kind, target, file, content, base_hash, note, status, made_by, made_by_project, \
                    made_by_session, created_at, decided_by, decided_at";

fn row(r: &Row) -> rusqlite::Result<Proposal> {
    let kind: String = r.get(1)?;
    let file: Option<String> = r.get(3)?;
    let (kind, file) = match (kind.as_str(), file) {
        ("widget", Some(f)) if f == FOLDER_FILE => (ProposalKind::Folder, None),
        ("widget", f) => (ProposalKind::Widget, f),
        (_, f) => (ProposalKind::Garden, f),
    };
    Ok(Proposal {
        id: r.get(0)?,
        kind,
        target: r.get(2)?,
        file,
        content: r.get(4)?,
        base_hash: r.get(5)?,
        note: r.get(6)?,
        status: r.get(7)?,
        made_by: r.get(8)?,
        made_by_project: r.get(9)?,
        made_by_session: r.get(10)?,
        created_at: r.get(11)?,
        decided_by: r.get(12)?,
        decided_at: r.get(13)?,
    })
}

/// Save a new pending proposal. A folder proposal names no `file` (its row
/// gets [`FOLDER_FILE`]); a widget one names a real file, never the mark.
pub fn insert(conn: &Connection, p: &NewProposal<'_>) -> rusqlite::Result<Proposal> {
    let id = format!("zp-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let file = match p.kind {
        ProposalKind::Folder => Some(FOLDER_FILE),
        _ if p.file == Some(FOLDER_FILE) => {
            return Err(rusqlite::Error::InvalidParameterName(format!("'{FOLDER_FILE}' marks a folder proposal; it is not a file")));
        }
        _ => p.file,
    };
    conn.execute(
        "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, note, status, made_by, \
         made_by_project, made_by_session, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9, ?10, ?11)",
        params![
            id,
            p.kind.code(),
            p.target,
            file,
            p.content,
            p.base_hash,
            p.note,
            p.made_by,
            p.made_by_project,
            p.made_by_session,
            p.now
        ],
    )?;
    get(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Proposal>> {
    conn.query_row(&format!("SELECT {COLS} FROM zen_proposals WHERE id = ?1"), params![id], row).optional()
}

/// Pending proposals (oldest first), or every proposal (newest first, at
/// most 200) with `all`.
pub fn list(conn: &Connection, all: bool) -> rusqlite::Result<Vec<Proposal>> {
    let sql = if all {
        format!("SELECT {COLS} FROM zen_proposals ORDER BY created_at DESC, id LIMIT 200")
    } else {
        format!("SELECT {COLS} FROM zen_proposals WHERE status = 'pending' ORDER BY created_at, id")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Close a pending proposal: `approved`, `rejected` or `withdrawn`.
/// `false` when it wasn't pending (someone else decided first).
pub fn decide(conn: &Connection, id: &str, status: &str, by: &str, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE zen_proposals SET status = ?2, decided_by = ?3, decided_at = ?4 WHERE id = ?1 AND status = 'pending'",
        params![id, status, by, now],
    )?;
    Ok(n == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_file_names_are_one_code_file() {
        for ok in ["index.html", "app.js", "style.css", "manifest.json", "board-2.js", "a_b.css"] {
            assert!(valid_widget_file(ok), "{ok}");
        }
        for bad in ["", ".env", "../x.js", "a/b.js", "x.png", "README.md", "Index.html", "x..js", "-a.js"] {
            assert!(!valid_widget_file(bad), "{bad}");
        }
    }

    #[test]
    fn hashes_mark_a_missing_file_empty() {
        assert_eq!(text_hash(None), "");
        assert_eq!(text_hash(Some("")).len(), 64);
        assert_ne!(text_hash(Some("a")), text_hash(Some("b")));
    }

    #[test]
    fn the_queue_inserts_lists_and_decides_once() {
        let _db = crate::db::scoped_for_test();
        let h = crate::db::shared();
        let conn = h.lock();
        let p = insert(
            &conn,
            &NewProposal {
                kind: ProposalKind::Widget,
                target: "board",
                file: Some("index.html"),
                content: "<p>hi</p>",
                base_hash: "",
                note: Some("first cut"),
                made_by: "agent:w-1",
                made_by_project: "w-1",
                made_by_session: Some("s-1"),
                now: 10,
            },
        )
        .expect("insert");
        assert_eq!(p.status, "pending");
        assert_eq!(list(&conn, false).expect("list").len(), 1);
        let v = p.summary();
        assert!(v.get("content").is_none(), "the list never carries the content: {v}");
        assert_eq!(v["bytes"], 9);
        assert!(decide(&conn, &p.id, "approved", "owner", 11).expect("decide"));
        assert!(!decide(&conn, &p.id, "rejected", "owner", 12).expect("decide"), "decided once");
        assert!(list(&conn, false).expect("list").is_empty());
        assert_eq!(list(&conn, true).expect("all").len(), 1);
        // A Garden proposal has no file; a widget one must.
        let bad = conn.execute(
            "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, status, made_by, made_by_project, created_at) \
             VALUES ('x', 'garden', 'g-1', 'index.html', '', '', 'pending', 'agent:w', 'w', 1)",
            [],
        );
        assert!(bad.is_err(), "a Garden proposal names no file");
    }

    #[test]
    fn a_folder_proposal_is_one_widget_row_marked_star_and_reads_back_as_folder() {
        use super::super::widget_folder::{FolderChange, FolderKeep, FolderProposal};
        let _db = crate::db::scoped_for_test();
        let h = crate::db::shared();
        let conn = h.lock();
        let fp = FolderProposal {
            files: vec![
                FolderChange { name: "app.js".into(), text: "x()".into(), base: text_hash(Some("y()")) },
                FolderChange { name: "index.html".into(), text: "<p>2</p>".into(), base: String::new() },
            ],
            keep: vec![FolderKeep { name: "logo.png".into(), hash: bytes_hash(Some(&[1, 2, 3])) }],
        };
        let content = serde_json::to_string(&fp).expect("encode");
        let p = insert(
            &conn,
            &NewProposal {
                kind: ProposalKind::Folder,
                target: "board",
                file: None,
                content: &content,
                base_hash: &fp.base_hash(),
                note: None,
                made_by: "agent:w-1",
                made_by_project: "w-1",
                made_by_session: None,
                now: 10,
            },
        )
        .expect("insert folder");
        assert_eq!((p.kind, p.file.as_deref()), (ProposalKind::Folder, None));
        let (kind, file): (String, Option<String>) = conn
            .query_row("SELECT kind, file FROM zen_proposals WHERE id = ?1", params![p.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("row");
        assert_eq!((kind.as_str(), file.as_deref()), ("widget", Some(FOLDER_FILE)), "no migration: a widget row marked *");
        assert_eq!(p.folder().expect("folder"), fp);
        let v = p.summary();
        assert_eq!((v["kind"].clone(), v["files"].clone(), v["unchanged"].clone()), (json!("folder"), json!(["app.js", "index.html"]), json!(1)), "{v}");
        assert!(v.get("content").is_none(), "{v}");
        // The mark is never a widget file of its own.
        let mark = insert(
            &conn,
            &NewProposal {
                kind: ProposalKind::Widget,
                target: "board",
                file: Some(FOLDER_FILE),
                content: "",
                base_hash: "",
                note: None,
                made_by: "agent:w-1",
                made_by_project: "w-1",
                made_by_session: None,
                now: 11,
            },
        );
        assert!(mark.is_err(), "a widget proposal can't name the folder mark");
        // A one-file widget proposal still reads back as a widget.
        let w = insert(
            &conn,
            &NewProposal {
                kind: ProposalKind::Widget,
                target: "board",
                file: Some("app.js"),
                content: "z()",
                base_hash: "",
                note: None,
                made_by: "agent:w-1",
                made_by_project: "w-1",
                made_by_session: None,
                now: 12,
            },
        )
        .expect("insert widget");
        assert_eq!((w.kind, w.file.as_deref()), (ProposalKind::Widget, Some("app.js")));
        assert!(w.folder().is_err());
    }

    #[test]
    fn a_folder_fingerprint_follows_every_file_in_any_order() {
        use super::super::widget_folder::{FolderChange, FolderKeep, FolderProposal};
        let a = FolderChange { name: "a.js".into(), text: "1".into(), base: "h1".into() };
        let b = FolderChange { name: "b.js".into(), text: "2".into(), base: String::new() };
        let k = FolderKeep { name: "c.css".into(), hash: "h3".into() };
        let one = FolderProposal { files: vec![a.clone(), b.clone()], keep: vec![k.clone()] };
        let two = FolderProposal { files: vec![b.clone(), a.clone()], keep: vec![k.clone()] };
        assert_eq!(one.base_hash(), two.base_hash());
        let moved = FolderProposal { files: vec![a, b], keep: vec![FolderKeep { name: "c.css".into(), hash: "h4".into() }] };
        assert_ne!(one.base_hash(), moved.base_hash(), "a kept file is part of the fingerprint");
    }
}
