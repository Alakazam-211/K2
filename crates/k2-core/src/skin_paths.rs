//! Folder path scope per room line (room-scope R3, amendment RS-A1..A3,
//! 0.46.1; skin.db step 5).
//!
//! A role's room line can name the folders a login of that role may reach
//! in that room, beside the line's caps:
//!
//! - `read`: folders the login may list and read (and everything under
//!   them);
//! - `write`: folders the login may also write (a write folder is
//!   readable too; the caps still decide: `files:write` is still needed
//!   to write, `files:read` to read);
//! - `uploadDir`: the default folder an upload lands in when the caller
//!   names none;
//! - `upload` (drop folders, prd-app-caps-0464-v1 AC8, 0.46.4): folders a
//!   login with `files:upload` may put files in (each lands in the
//!   uploader's own subfolder) and can never list or read through this
//!   list (only through a `read` or `write` entry, if the line has one);
//! - `download` (download entries, AC15, 0.46.4): folders **or single
//!   files** a login with `files:download` may save without `files:read`
//!   (`files.downloads` / `files.download`). An empty list exposes
//!   nothing (never "the whole room"), and download entries never wall the
//!   line's read/write access ([`RoomPathScope::is_scoped`] ignores them).
//!   Adding or widening them is person-only (the daemon's paths route).
//!
//! **No folders listed = the whole room** (today's behaviour). Once a room
//! line lists any folder (read, write or drop), everything else in that
//! room is refused **with
//! the same 403 a missing cap gets** (RS-A3), checked on the room-relative
//! path before the disk is touched, so an unlisted path never says whether
//! it exists. The folders of the room's ancestors are listable only down
//! to the granted folders (a listing shows the way in, never a sibling).
//!
//! RS-A1: the scope is per role **per room line**, so it works on any room
//! the role holds, not only a login's "home" room. RS-A2: it follows the
//! role, so an agent login (`k2 app user agent`) and a person get exactly
//! the same walls. RS-A4 (known gap): a native agent reading another
//! room's folder by disk path is not walled; its walled route is its
//! marked App login.
//!
//! Stored on `roles.room_paths` (JSON `{project_id: {read, write,
//! uploadDir}}`) and read from the session's own grant → role every time
//! its pass is resolved ([`session_paths`], onto [`SkinPass::room_paths`]),
//! so an edit applies from a live session's next request. A scope that
//! can't be read is [`PassPaths::Unreadable`] and every file door fails
//! closed on it.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{
    attach_role_handles, blank_to_none, enabled_app_grant, normalize_room_ids, role_by_id_or_name, with_conn,
    SkinPass, SkinRole, HOST_APP_ID,
};

/// At most this many folders per list on one room line.
pub const MAX_FOLDERS_PER_LIST: usize = 200;
/// At most this many characters in one folder.
pub const MAX_FOLDER_LEN: usize = 1024;

/// One room line's folders. Empty lists = the whole room.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoomPathScope {
    /// Folders the login may list and read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    /// Folders the login may write (and read).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<String>,
    /// Where an upload lands when the caller names no folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_dir: Option<String>,
    /// Drop folders (AC8): `files:upload` may add files here, never list or
    /// read them. Counts towards [`Self::is_scoped`]: a line that lists only
    /// drop folders walls the rest of the room. An older K2 refuses this
    /// key (`deny_unknown_fields`), so after a downgrade the role reads as
    /// unreadable and every file door fails closed for it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upload: Vec<String>,
    /// Download entries (AC15): folders or single files `files:download`
    /// may save without `files:read`. Empty = nothing. Not part of
    /// [`Self::is_scoped`]. An older K2 refuses this key too (fails closed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub download: Vec<String>,
}

impl RoomPathScope {
    /// `true` when the line lists any folder (the room is walled).
    pub fn is_scoped(&self) -> bool {
        !self.read.is_empty() || !self.write.is_empty() || !self.upload.is_empty()
    }

    /// `true` when there is nothing to store (no folders, no upload dir,
    /// no download entries).
    pub fn is_empty(&self) -> bool {
        !self.is_scoped() && self.upload_dir.is_none() && self.download.is_empty()
    }
}

/// `project_id → folders` on one role.
pub type RoomPaths = BTreeMap<String, RoomPathScope>;

/// A resolved pass's folders ([`SkinPass::room_paths`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PassPaths {
    /// No role (platform, link, role-less grant): every line is the whole
    /// room.
    #[default]
    Whole,
    /// The role's folders per room line (a line not listed is the whole
    /// room).
    Lines(RoomPaths),
    /// The scope couldn't be read (grant, role or stored JSON): fail closed.
    Unreadable(String),
}

/// What a file door needs on a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathNeed {
    /// List or stat: inside a read or write folder, or a folder on the way
    /// to one (the listing is filtered to the way in).
    List,
    /// Read the bytes: inside a read or write folder.
    Read,
    /// Create, write, upload, rename or move: inside a write folder.
    Write,
    /// Add a new file through an upload door with `files:upload` (AC9):
    /// inside a write folder or a drop folder. `List`, `Read` and `Write`
    /// never match a drop-only folder.
    Upload,
}

/// Segments of a room-relative request path. `.` and empty segments drop
/// (`.` alone is the room root, `[]`). `/` and `\` both separate. The
/// errors are the jail's own words ([`jail_rel_path`] in k2-daemon), so a
/// scoped login and an unscoped one see the same 400 for a bad path.
pub fn rel_segments(rel: &str) -> Result<Vec<String>, String> {
    let t = rel.trim();
    if t.is_empty() {
        return Err("empty path".to_string());
    }
    if t.starts_with('/') || t.starts_with('\\') || has_drive_prefix(t) {
        return Err("path must be relative to the workspace".to_string());
    }
    let mut out = Vec::new();
    for seg in t.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => return Err("path must not contain '..'".to_string()),
            s => out.push(s.to_string()),
        }
    }
    Ok(out)
}

fn has_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A folder as stored: room-relative, `/`-joined, no `.`/`..`, never the
/// room root (the root is "no folders listed").
pub fn normalize_folder(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.chars().count() > MAX_FOLDER_LEN {
        return Err(format!("folder is longer than {MAX_FOLDER_LEN} characters"));
    }
    if t.contains('\0') {
        return Err(format!("folder {raw:?} has a NUL character"));
    }
    let segs = rel_segments(t).map_err(|e| format!("folder {raw:?}: {e}"))?;
    if segs.is_empty() {
        return Err(format!(
            "folder {raw:?} is the room root; leave the room line without folders for the whole room"
        ));
    }
    Ok(segs.join("/"))
}

fn folder_segments(folder: &str) -> Vec<&str> {
    folder.split('/').filter(|s| !s.is_empty()).collect()
}

/// `rel` is the folder or under it.
fn is_inside(folder: &str, rel: &[String]) -> bool {
    let f = folder_segments(folder);
    rel.len() >= f.len() && f.iter().zip(rel.iter()).all(|(a, b)| *a == b.as_str())
}

/// `rel` is a strict ancestor of the folder (the room root counts).
fn is_ancestor(folder: &str, rel: &[String]) -> bool {
    let f = folder_segments(folder);
    rel.len() < f.len() && rel.iter().zip(f.iter()).all(|(a, b)| a.as_str() == *b)
}

fn readable(scope: &RoomPathScope) -> impl Iterator<Item = &String> {
    scope.read.iter().chain(scope.write.iter())
}

/// May a login with `scope` do `need` on the room-relative `rel`
/// (already split by [`rel_segments`])? Unscoped lines allow everything
/// (the caps still decide).
pub fn allows(scope: &RoomPathScope, rel: &[String], need: PathNeed) -> bool {
    if !scope.is_scoped() {
        return true;
    }
    match need {
        PathNeed::Write => scope.write.iter().any(|f| is_inside(f, rel)),
        PathNeed::Upload => scope.write.iter().chain(scope.upload.iter()).any(|f| is_inside(f, rel)),
        PathNeed::Read => readable(scope).any(|f| is_inside(f, rel)),
        PathNeed::List => readable(scope).any(|f| is_inside(f, rel) || is_ancestor(f, rel)),
    }
}

/// The drop root an upload-only drop at `rel` (already split) lands under:
/// the OUTERMOST write or drop folder holding `rel` (0.46.4 review A1).
/// The file then lands in `<root>/<username>/<rest of rel>`, so the
/// uploader's own name is always the first segment under the root and no
/// `dir` can reach another person's subfolder. `None` = outside every
/// write and drop folder.
pub fn drop_root(scope: &RoomPathScope, rel: &[String]) -> Option<String> {
    scope
        .write
        .iter()
        .chain(scope.upload.iter())
        .filter(|f| is_inside(f, rel))
        .min_by_key(|f| folder_segments(f).len())
        .cloned()
}

/// Is `folder` the room-relative `dir` (already split) or inside it?
/// (Round 2 A1: a guest's `<root>/<user>` must not be, or hold, a folder
/// some role lists.)
pub fn folder_holds(dir: &[String], folder: &str) -> bool {
    let f = folder_segments(folder);
    f.len() >= dir.len() && dir.iter().zip(f.iter()).all(|(a, b)| a.as_str() == *b)
}

/// Is the room-relative `rel` (already split) inside one of the line's
/// download entries (AC15)? A file entry matches itself exactly (the same
/// segment rule as a folder: `reports/a.pdf` never matches
/// `reports/a.pdf.bak`), a folder entry everything under it. An empty
/// list is nothing.
pub fn download_allows(scope: &RoomPathScope, rel: &[String]) -> bool {
    scope.download.iter().any(|e| is_inside(e, rel))
}

/// The first entry of `new` that no entry of `old` covers (the entry or
/// one of its parents is in `old`): what a widening adds. `None` = `new`
/// only narrows or keeps (AC18, person-only widening).
pub fn download_widening<'a>(old: &[String], new: &'a [String]) -> Option<&'a str> {
    new.iter()
        .find(|n| {
            let segs: Vec<String> = folder_segments(n).into_iter().map(str::to_string).collect();
            !old.iter().any(|o| is_inside(o, &segs))
        })
        .map(String::as_str)
}

/// [`allows`] on a raw room-relative path. `Err` = a malformed path (the
/// jail's own 400 words).
pub fn allows_rel(scope: &RoomPathScope, rel: &str, need: PathNeed) -> Result<bool, String> {
    if !scope.is_scoped() {
        return Ok(true);
    }
    Ok(allows(scope, &rel_segments(rel)?, need))
}

/// `true` when a folder names the agent's config folder `.k2` or sits
/// inside it: its first segment, ASCII case-folded, trailing dots and
/// spaces ignored (the daemon's `is_agent_config_segment` rule).
pub fn folder_in_agent_config(folder: &str) -> bool {
    folder_segments(folder).first().is_some_and(|s| s.trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".k2"))
}

/// Normalize, dedupe and check one room line's folders.
pub fn normalize_scope(scope: &RoomPathScope) -> Result<RoomPathScope, String> {
    let list = |raw: &[String], what: &str| -> Result<Vec<String>, String> {
        if raw.len() > MAX_FOLDERS_PER_LIST {
            return Err(format!("at most {MAX_FOLDERS_PER_LIST} {what} folders per room"));
        }
        let mut out: Vec<String> = Vec::new();
        for f in raw {
            let n = normalize_folder(f)?;
            if !out.contains(&n) {
                out.push(n);
            }
        }
        Ok(out)
    };
    let write = list(&scope.write, "write")?;
    // A folder listed for write is already readable; drop the duplicate.
    let read: Vec<String> = list(&scope.read, "read")?.into_iter().filter(|f| !write.contains(f)).collect();
    let upload_dir = match scope.upload_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => Some(normalize_folder(raw).map_err(|e| format!("uploadDir: {e}"))?),
        None => None,
    };
    // A drop folder that is also a write folder adds nothing; drop it.
    let upload: Vec<String> = list(&scope.upload, "upload")?.into_iter().filter(|f| !write.contains(f)).collect();
    if let Some(f) = upload.iter().find(|f| folder_in_agent_config(f)) {
        return Err(format!("upload folder {f:?} is the agent's .k2 folder or inside it; a drop folder never is"));
    }
    // Download entries: folders or files, normalized like folders; never
    // the agent's .k2 (AC19).
    let download = list(&scope.download, "download")?;
    if let Some(f) = download.iter().find(|f| folder_in_agent_config(f)) {
        return Err(format!("download entry {f:?} is the agent's .k2 folder or inside it; it can never be downloaded"));
    }
    let out = RoomPathScope { read, write, upload_dir, upload, download };
    if let (true, Some(dir)) = (out.is_scoped(), out.upload_dir.as_deref()) {
        let segs: Vec<String> = dir.split('/').map(str::to_string).collect();
        // AC8: inside a write folder or a drop folder.
        if !allows(&out, &segs, PathNeed::Upload) {
            let places: Vec<String> = out.write.iter().chain(out.upload.iter()).cloned().collect();
            return Err(format!(
                "uploadDir {dir:?} must be inside one of this room's write or upload folders ({})",
                if places.is_empty() { "none listed".to_string() } else { places.join(", ") }
            ));
        }
    }
    Ok(out)
}

/// Parse a stored `roles.room_paths`. Strict: anything unreadable is an
/// error, and every reader of a scope fails closed on it.
pub fn parse_room_paths(raw: Option<&str>) -> Result<RoomPaths, String> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(RoomPaths::new());
    };
    let parsed: BTreeMap<String, RoomPathScope> =
        serde_json::from_str(s).map_err(|e| format!("room_paths json: {e}"))?;
    let mut out = RoomPaths::new();
    for (room, scope) in parsed {
        let room = room.trim().to_string();
        if room.is_empty() {
            return Err("room_paths: empty room id".to_string());
        }
        let n = normalize_scope(&scope).map_err(|e| format!("room_paths {room}: {e}"))?;
        if !n.is_empty() {
            out.insert(room, n);
        }
    }
    Ok(out)
}

pub(super) fn room_paths_json(paths: &RoomPaths) -> String {
    serde_json::to_string(paths).unwrap_or_else(|_| "{}".to_string())
}

/// The stored folders of one role (strict).
pub(super) fn role_room_paths(conn: &Connection, role_id: &str) -> Result<RoomPaths, String> {
    let raw: Option<String> = conn
        .query_row("SELECT room_paths FROM roles WHERE id = ?1", params![role_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("skin role room_paths: {e}"))?
        .ok_or_else(|| format!("unknown skin role '{role_id}'"))?;
    parse_room_paths(raw.as_deref())
}

pub(super) fn persist_role_paths(conn: &Connection, role_id: &str, paths: &RoomPaths) -> Result<(), String> {
    conn.execute(
        "UPDATE roles SET room_paths = ?1 WHERE id = ?2",
        params![room_paths_json(paths), role_id],
    )
    .map_err(|e| format!("skin role room_paths update: {e}"))?;
    Ok(())
}

/// Drop the folders of rooms the role no longer holds.
pub(super) fn prune_role_paths(conn: &Connection, role: &SkinRole) -> Result<(), String> {
    let mut paths = role_room_paths(conn, &role.id)?;
    let before = paths.len();
    paths.retain(|room, _| role.room_policy.contains_key(room));
    if paths.len() != before {
        persist_role_paths(conn, &role.id, &paths)?;
    }
    Ok(())
}

/// Step 5 (room-scope R3): `roles.room_paths`. Additive; `{}` = every room
/// line unscoped (the whole room), which is what every role had before.
pub(super) fn step5_room_paths_column(conn: &Connection) -> Result<(), String> {
    if !super::column_exists(conn, "roles", "room_paths")? {
        conn.execute("ALTER TABLE roles ADD COLUMN room_paths TEXT NOT NULL DEFAULT '{}'", [])
            .map_err(|e| format!("skin.db step 5 roles: {e}"))?;
    }
    Ok(())
}

/// Why [`update_role_room_paths`] wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsUpdateError {
    /// The caller's rule refused the change (the daemon's person-only
    /// download widening: the entry it would add).
    Refused(String),
    /// A bad role, room or folder, or storage.
    Bad(String),
}

/// Read one room line's stored folders, let `f` build the new line from
/// them (or refuse), and store it: ONE skin.db transaction under the store
/// lock, so no other writer lands between the read and the write (0.46.4
/// review A5: the download-widening check sees exactly what it replaces).
pub fn update_role_room_paths<F>(id_or_name: &str, project_id: &str, f: F) -> Result<SkinRole, PathsUpdateError>
where
    F: FnOnce(&RoomPathScope) -> Result<RoomPathScope, PathsUpdateError>,
{
    let rooms = normalize_room_ids(std::slice::from_ref(&project_id.to_string()));
    let Some(project_id) = rooms.into_iter().next() else {
        return Err(PathsUpdateError::Bad("missing workspace".to_string()));
    };
    let mut refused: Option<PathsUpdateError> = None;
    let out = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin role paths: {e}"))?;
        let Some(mut role) = role_by_id_or_name(&tx, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        if !role.room_policy.contains_key(&project_id) {
            return Err(format!(
                "role '{}' has no line for that room; add it first with k2 app role room {} <room> --caps …",
                role.name, role.name
            ));
        }
        let mut paths = role_room_paths(&tx, &role.id)?;
        let stored = paths.get(&project_id).cloned().unwrap_or_default();
        let next = match f(&stored) {
            Ok(n) => normalize_scope(&n)?,
            Err(e) => {
                refused = Some(e);
                return Ok(None);
            }
        };
        if next.is_empty() {
            paths.remove(&project_id);
        } else {
            paths.insert(project_id.clone(), next);
        }
        persist_role_paths(&tx, &role.id, &paths)?;
        tx.commit().map_err(|e| format!("skin role paths commit: {e}"))?;
        role.room_paths = paths;
        Ok(Some(role))
    })
    .map_err(PathsUpdateError::Bad)?;
    match (out, refused) {
        (Some(role), _) => {
            crate::workspace::context_layers::refresh_skin_roster_after_people_change();
            Ok(attach_role_handles(role))
        }
        (None, Some(e)) => Err(e),
        (None, None) => Err(PathsUpdateError::Bad("skin role paths: nothing written".to_string())),
    }
}

/// Set one room line's folders on a role (`k2 app role paths`). The room
/// must already be on the role (`k2 app role room` first). An empty scope
/// clears the line (back to the whole room).
pub fn set_role_room_paths(id_or_name: &str, project_id: &str, scope: &RoomPathScope) -> Result<SkinRole, String> {
    let rooms = normalize_room_ids(std::slice::from_ref(&project_id.to_string()));
    let Some(project_id) = rooms.into_iter().next() else {
        return Err("missing workspace".to_string());
    };
    let scope = normalize_scope(scope)?;
    let role = with_conn(|conn| {
        let Some(mut role) = role_by_id_or_name(conn, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        if !role.room_policy.contains_key(&project_id) {
            return Err(format!(
                "role '{}' has no line for that room; add it first with k2 app role room {} <room> --caps …",
                role.name, role.name
            ));
        }
        let mut paths = role_room_paths(conn, &role.id)?;
        if scope.is_empty() {
            paths.remove(&project_id);
        } else {
            paths.insert(project_id.clone(), scope.clone());
        }
        persist_role_paths(conn, &role.id, &paths)?;
        role.room_paths = paths;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

/// The folders a session login holds, from its own `(principal, App)`
/// grant → role (S24: the grant is the login truth). A role-less grant
/// (rooms, Thread-only) is [`PassPaths::Whole`]. Anything that can't be
/// read (no principal, no enabled grant, a missing role, a stored scope
/// that doesn't parse) is [`PassPaths::Unreadable`]: never "whole room".
pub(super) fn session_paths(conn: &Connection, pass: &SkinPass) -> PassPaths {
    match session_paths_inner(conn, pass) {
        Ok(p) => p,
        Err(e) => PassPaths::Unreadable(e),
    }
}

fn session_paths_inner(conn: &Connection, pass: &SkinPass) -> Result<PassPaths, String> {
    let Some(principal) = pass.principal_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err("path scope: session pass has no principal".to_string());
    };
    let app = blank_to_none(Some(pass.app.clone())).unwrap_or_else(|| HOST_APP_ID.to_string());
    let Some(grant) = enabled_app_grant(conn, principal, &app)? else {
        return Err("path scope: no enabled grant for the session's App".to_string());
    };
    let Some(role_ref) = blank_to_none(grant.role_id) else {
        return Ok(PassPaths::Whole);
    };
    let Some(role) = role_by_id_or_name(conn, &role_ref)? else {
        return Err(format!("path scope: the grant's role '{role_ref}' is gone"));
    };
    Ok(PassPaths::Lines(role_room_paths(conn, &role.id)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(s: &str) -> Vec<String> {
        rel_segments(s).expect("valid path")
    }

    fn scope(read: &[&str], write: &[&str]) -> RoomPathScope {
        normalize_scope(&RoomPathScope {
            read: read.iter().map(|s| s.to_string()).collect(),
            write: write.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
        .expect("valid scope")
    }

    #[test]
    fn unscoped_allows_everything() {
        let s = RoomPathScope::default();
        for need in [PathNeed::List, PathNeed::Read, PathNeed::Write] {
            assert!(allows(&s, &segs("."), need));
            assert!(allows(&s, &segs("any/where.txt"), need));
        }
    }

    #[test]
    fn read_write_and_list_rules() {
        let s = scope(&["jobs/J1/", "jobs/J3"], &["jobs/J1/uploads"]);
        // Read folders and below.
        assert!(allows(&s, &segs("jobs/J1"), PathNeed::Read));
        assert!(allows(&s, &segs("jobs/J1/plans/a.pdf"), PathNeed::Read));
        assert!(allows(&s, &segs("./jobs//J3/x"), PathNeed::Read));
        assert!(!allows(&s, &segs("jobs/J2/x"), PathNeed::Read));
        assert!(!allows(&s, &segs("jobs/J10/x"), PathNeed::Read), "a prefix of a name is not the folder");
        assert!(!allows(&s, &segs("jobs"), PathNeed::Read), "an ancestor is never readable");
        assert!(!allows(&s, &segs("jobs/j1/x"), PathNeed::Read), "names are exact");
        // Write only inside write folders; a write folder is readable.
        assert!(allows(&s, &segs("jobs/J1/uploads/p.jpg"), PathNeed::Write));
        assert!(allows(&s, &segs("jobs/J1/uploads"), PathNeed::Read));
        assert!(!allows(&s, &segs("jobs/J1/plans/a.pdf"), PathNeed::Write));
        assert!(!allows(&s, &segs("jobs/J3/x"), PathNeed::Write));
        // List: the way in, never a sibling.
        assert!(allows(&s, &segs("."), PathNeed::List));
        assert!(allows(&s, &segs("jobs"), PathNeed::List));
        assert!(allows(&s, &segs("jobs/J1/plans"), PathNeed::List));
        assert!(!allows(&s, &segs("jobs/J2"), PathNeed::List));
        assert!(!allows(&s, &segs("other"), PathNeed::List));
    }

    #[test]
    fn malformed_paths_use_the_jails_words() {
        let s = scope(&["a"], &[]);
        assert_eq!(allows_rel(&s, "a/../b", PathNeed::Read).unwrap_err(), "path must not contain '..'");
        assert_eq!(allows_rel(&s, "/etc", PathNeed::Read).unwrap_err(), "path must be relative to the workspace");
        assert_eq!(allows_rel(&s, "  ", PathNeed::Read).unwrap_err(), "empty path");
        assert_eq!(allows_rel(&s, "C:/x", PathNeed::Read).unwrap_err(), "path must be relative to the workspace");
        assert!(allows_rel(&s, "a\\b", PathNeed::Read).unwrap(), "a backslash separates too");
    }

    #[test]
    fn normalize_scope_rules() {
        let n = scope(&["jobs/J1/", "jobs/J1", "./jobs/J2"], &["jobs/J2"]);
        assert_eq!(n.read, ["jobs/J1"], "deduped; a write folder leaves the read list");
        assert_eq!(n.write, ["jobs/J2"]);
        for bad in [".", "", "/abs", "a/../b", "..", "C:\\x"] {
            let r = normalize_scope(&RoomPathScope { read: vec![bad.to_string()], ..Default::default() });
            assert!(r.is_err(), "{bad:?} must be refused");
        }
        let up = |dir: &str, write: &[&str], read: &[&str]| {
            normalize_scope(&RoomPathScope {
                read: read.iter().map(|s| s.to_string()).collect(),
                write: write.iter().map(|s| s.to_string()).collect(),
                upload_dir: Some(dir.to_string()),
                ..Default::default()
            })
        };
        assert_eq!(up("jobs/J1/in/", &["jobs/J1"], &[]).unwrap().upload_dir.as_deref(), Some("jobs/J1/in"));
        assert!(up("jobs/J2", &["jobs/J1"], &[]).is_err(), "outside the write folders");
        assert!(up("jobs/J1", &[], &["jobs/J1"]).is_err(), "a read-only line has nowhere to upload");
        let err = up("jobs/J1", &[], &["jobs/J1"]).unwrap_err();
        assert!(err.contains("write or upload folders"), "{err}");
        // No folders: the upload dir is only a default (the whole room is open).
        assert_eq!(up("inbox", &[], &[]).unwrap().upload_dir.as_deref(), Some("inbox"));
        assert!(!up("inbox", &[], &[]).unwrap().is_scoped());
        let many: Vec<String> = (0..=MAX_FOLDERS_PER_LIST).map(|i| format!("f{i}")).collect();
        assert!(normalize_scope(&RoomPathScope { read: many, ..Default::default() }).is_err());
    }

    fn policy(lines: &[(&str, &[&str])]) -> super::super::RoomPolicy {
        lines
            .iter()
            .map(|(room, caps)| (room.to_string(), caps.iter().map(|c| c.to_string()).collect()))
            .collect()
    }

    fn folders(read: &[&str], write: &[&str], upload: Option<&str>) -> RoomPathScope {
        RoomPathScope {
            read: read.iter().map(|s| s.to_string()).collect(),
            write: write.iter().map(|s| s.to_string()).collect(),
            upload_dir: upload.map(str::to_string),
            ..Default::default()
        }
    }

    fn drop_scope(read: &[&str], write: &[&str], upload: &[&str], dir: Option<&str>) -> Result<RoomPathScope, String> {
        normalize_scope(&RoomPathScope {
            read: read.iter().map(|s| s.to_string()).collect(),
            write: write.iter().map(|s| s.to_string()).collect(),
            upload: upload.iter().map(|s| s.to_string()).collect(),
            upload_dir: dir.map(str::to_string),
            ..Default::default()
        })
    }

    fn dl(read: &[&str], download: &[&str]) -> Result<RoomPathScope, String> {
        normalize_scope(&RoomPathScope {
            read: read.iter().map(|s| s.to_string()).collect(),
            download: download.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
    }

    /// AC15: download entries hold folders or single files; an empty list
    /// is nothing; they never wall read/write.
    #[test]
    fn download_entries_are_exact_and_never_wall_the_line() {
        let none = dl(&[], &[]).unwrap();
        assert!(!download_allows(&none, &segs("a.pdf")), "an empty list exposes nothing");
        assert!(!download_allows(&none, &segs(".")));
        let s = dl(&[], &["reports/2026-10.pdf", "public/"]).unwrap();
        assert_eq!(s.download, ["reports/2026-10.pdf", "public"]);
        assert!(download_allows(&s, &segs("reports/2026-10.pdf")));
        assert!(!download_allows(&s, &segs("reports/2026-11.pdf")), "a sibling of a file entry");
        assert!(!download_allows(&s, &segs("reports/2026-10.pdf.bak")));
        assert!(!download_allows(&s, &segs("reports")), "the file's folder isn't the file");
        assert!(download_allows(&s, &segs("public/a/b.txt")));
        assert!(!download_allows(&s, &segs("publicity/x")));
        // Never walls: no read/write folders = still the whole room.
        assert!(!s.is_scoped());
        assert!(!s.is_empty(), "there is something to store");
        for need in [PathNeed::List, PathNeed::Read, PathNeed::Write] {
            assert!(allows(&s, &segs("anything/else"), need), "{need:?}");
        }
        // With read folders the walls are the read folders' only.
        let s = dl(&["jobs/J1"], &["reports/a.pdf"]).unwrap();
        assert!(!allows(&s, &segs("reports/a.pdf"), PathNeed::Read), "a download entry is never a read");
        assert!(allows(&s, &segs("jobs/J1/x"), PathNeed::Read));
        // .k2 entries are refused when set (AC19).
        for bad in [".k2", ".K2/config.toml", ".k2./x"] {
            assert!(dl(&[], &[bad]).unwrap_err().contains(".k2"), "{bad:?}");
        }
        assert!(dl(&[], &["../x"]).is_err());
        assert!(parse_room_paths(Some(r#"{"r1":{"download":["a/b.pdf"]}}"#)).unwrap()["r1"].download == ["a/b.pdf"]);
    }

    #[test]
    fn folder_holds_is_the_folder_or_below_it() {
        assert!(folder_holds(&segs("Inbox/vip"), "Inbox/vip"));
        assert!(folder_holds(&segs("Inbox/vip"), "Inbox/vip/deep"));
        assert!(!folder_holds(&segs("Inbox/vip"), "Inbox"));
        assert!(!folder_holds(&segs("Inbox/vip"), "Inbox/vipx"));
        assert!(!folder_holds(&segs("Inbox/dana"), "Inbox/vip"));
    }

    #[test]
    fn drop_root_is_the_outermost_folder_holding_the_path() {
        let s = drop_scope(&[], &["Work"], &["Inbox", "Inbox/vip"], None).unwrap();
        assert_eq!(drop_root(&s, &segs("Inbox")).as_deref(), Some("Inbox"));
        assert_eq!(drop_root(&s, &segs("Inbox/vip/x")).as_deref(), Some("Inbox"), "outermost, so a user's folder is always first");
        assert_eq!(drop_root(&s, &segs("Inbox/dana")).as_deref(), Some("Inbox"));
        assert_eq!(drop_root(&s, &segs("Work/a")).as_deref(), Some("Work"));
        assert_eq!(drop_root(&s, &segs("Other")), None);
        assert_eq!(drop_root(&s, &segs(".")), None);
    }

    #[test]
    fn download_widening_finds_the_first_uncovered_entry() {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(download_widening(&v(&[]), &v(&[])), None);
        assert_eq!(download_widening(&v(&["reports"]), &v(&["reports/a.pdf", "reports"])), None, "inside a stored folder");
        assert_eq!(download_widening(&v(&["reports/a.pdf", "x"]), &v(&["reports/a.pdf"])), None, "narrowing");
        assert_eq!(download_widening(&v(&["reports/a.pdf"]), &v(&["reports"])), Some("reports"), "a file to its folder widens");
        assert_eq!(download_widening(&v(&["reports"]), &v(&["reports", "other.txt"])), Some("other.txt"));
        assert_eq!(download_widening(&v(&[]), &v(&["a"])), Some("a"));
    }

    /// AC8/AC9: a drop folder takes uploads only. It is never listable or
    /// readable through the drop list, it walls the rest of the room, and
    /// `List`, `Read` and `Write` never match it.
    #[test]
    fn drop_folders_take_uploads_and_nothing_else() {
        let s = drop_scope(&[], &[], &["Inbox/"], None).expect("valid");
        assert_eq!(s.upload, ["Inbox"]);
        assert!(s.is_scoped(), "a line with only drop folders is walled");
        assert!(!s.is_empty());
        assert!(allows(&s, &segs("Inbox/dana/a.pdf"), PathNeed::Upload));
        assert!(allows(&s, &segs("Inbox"), PathNeed::Upload));
        for need in [PathNeed::List, PathNeed::Read, PathNeed::Write] {
            assert!(!allows(&s, &segs("Inbox"), need), "{need:?} on the drop folder");
            assert!(!allows(&s, &segs("Inbox/dana/a.pdf"), need), "{need:?} inside the drop folder");
            assert!(!allows(&s, &segs("other/x"), need), "{need:?}: the rest of the room is walled");
        }
        assert!(!allows(&s, &segs("."), PathNeed::List), "the drop folder is not even on the way in");
        assert!(!allows(&s, &segs("Inboxes/x"), PathNeed::Upload), "a prefix of a name is not the folder");
        assert!(!allows(&s, &segs("other/x"), PathNeed::Upload));
        // A write folder takes uploads too; a read folder doesn't.
        let s = drop_scope(&["Plans"], &["Work"], &["Drop"], None).expect("valid");
        assert!(allows(&s, &segs("Work/a"), PathNeed::Upload));
        assert!(!allows(&s, &segs("Plans/a"), PathNeed::Upload));
        assert!(allows(&s, &segs("Plans/a"), PathNeed::Read));
        assert!(!allows(&s, &segs("Drop/a"), PathNeed::Read), "read never reaches a drop folder");
        // A drop folder inside a read folder is readable through the read entry.
        let s = drop_scope(&["Shared"], &[], &["Shared/Inbox"], None).expect("valid");
        assert!(allows(&s, &segs("Shared/Inbox/a"), PathNeed::Read));
    }

    #[test]
    fn drop_folder_rules_when_set() {
        // uploadDir may sit inside a drop folder (AC8 relaxes R3).
        assert_eq!(drop_scope(&[], &[], &["Inbox"], Some("Inbox")).unwrap().upload_dir.as_deref(), Some("Inbox"));
        assert_eq!(drop_scope(&[], &[], &["Inbox"], Some("Inbox/new")).unwrap().upload_dir.as_deref(), Some("Inbox/new"));
        assert!(drop_scope(&[], &[], &["Inbox"], Some("Elsewhere")).is_err());
        // .k2 (any case, trailing dots or spaces) is never a drop folder.
        for bad in [".k2", ".K2", ".k2/inbox", ".k2.", ".k2 /x"] {
            let err = drop_scope(&[], &[], &[bad], None).unwrap_err();
            assert!(err.contains(".k2"), "{bad:?}: {err}");
        }
        assert!(drop_scope(&[], &[], &["docs/.k2"], None).is_ok(), "only the room's own .k2 is the agent's");
        // A drop folder that is also a write folder is dropped (write covers it).
        let s = drop_scope(&[], &["Inbox"], &["Inbox", "Drop", "Drop/"], None).unwrap();
        assert_eq!(s.upload, ["Drop"]);
        // Same normalization and limits as read/write.
        assert!(drop_scope(&[], &[], &["../x"], None).is_err());
        assert!(drop_scope(&[], &[], &["."], None).is_err());
        let many: Vec<String> = (0..=MAX_FOLDERS_PER_LIST).map(|i| format!("f{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(drop_scope(&[], &[], &many, None).is_err());
    }

    /// RS-A1: folders on ANY room line of the role (here `docs`, while the
    /// login's other line is `desk`), read from the session's own grant →
    /// role whenever its pass resolves: an edit reaches a live session on
    /// its next request, and dropping the room drops its folders.
    #[test]
    fn scope_is_per_room_line_and_read_live() {
        use super::super::*;
        let _home = crate::test_env::TempHome::new();
        let desk = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        create_role(
            "plans-desk",
            &policy(&[(&desk, &[CAP_THREAD_READ]), (&docs, &[CAP_FILES_READ])]),
        )
        .unwrap();
        add_principal("deskbot").unwrap();
        assign_role("deskbot", "plans-desk").unwrap();
        let (_, raw) = create_session_token("deskbot").unwrap();
        let pass = resolve_skin_token(&raw).expect("live session");
        assert_eq!(pass.path_scope(&docs).unwrap(), RoomPathScope::default(), "unscoped until set");

        let role = set_role_room_paths("plans-desk", &docs, &folders(&["jobs/J1/", "jobs/J3"], &[], None)).unwrap();
        assert_eq!(role.room_paths[&docs].read, ["jobs/J1", "jobs/J3"]);
        // The same live session, on its next request (the pass re-resolves).
        let pass = resolve_skin_token(&raw).expect("still live");
        let live = pass.path_scope(&docs).unwrap();
        assert_eq!(live.read, ["jobs/J1", "jobs/J3"], "the same live session sees the edit");
        assert!(allows(&live, &segs("jobs/J1/plan.pdf"), PathNeed::Read));
        assert!(!allows(&live, &segs("jobs/J2/plan.pdf"), PathNeed::Read));
        assert_eq!(pass.path_scope(&desk).unwrap(), RoomPathScope::default(), "the other line is untouched");
        // A role listing (the wire) carries the folders on that line only.
        let listed = list_roles().unwrap().into_iter().find(|r| r.name == "plans-desk").unwrap();
        assert_eq!(listed.room_paths.len(), 1);
        assert!(listed.room_paths.contains_key(&docs));

        // A room the role doesn't hold can't get folders.
        let other = uuid::Uuid::new_v4().to_string();
        let err = set_role_room_paths("plans-desk", &other, &folders(&["a"], &[], None)).unwrap_err();
        assert!(err.contains("no line for that room"), "{err}");

        // Dropping the room drops its folders: re-adding starts unscoped.
        clear_role_room("plans-desk", &docs).unwrap();
        set_role_room("plans-desk", &docs, Some(&[CAP_FILES_READ.to_string()])).unwrap();
        let again = find_role("plans-desk").unwrap().unwrap();
        assert!(again.room_paths.is_empty(), "no stale scope after re-adding the room");

        // An empty scope clears; a platform pass is never scoped.
        set_role_room_paths("plans-desk", &docs, &folders(&["jobs/J1"], &[], None)).unwrap();
        set_role_room_paths("plans-desk", &docs, &RoomPathScope::default()).unwrap();
        assert!(find_role("plans-desk").unwrap().unwrap().room_paths.is_empty());
        let (_, platform) = create_token("bff", Some(&[CAP_FILES_READ.to_string()]), &[docs.clone()]).unwrap();
        let platform = resolve_skin_token(&platform).unwrap();
        assert_eq!(platform.room_paths, PassPaths::Whole);
        assert_eq!(platform.path_scope(&docs).unwrap(), RoomPathScope::default());
    }

    /// Review A5: one transaction; a refusal from the caller's rule writes
    /// nothing, and the rule sees exactly the stored line it replaces.
    #[test]
    fn update_role_room_paths_reads_checks_and_writes_at_once() {
        use super::super::*;
        let _home = crate::test_env::TempHome::new();
        let docs = uuid::Uuid::new_v4().to_string();
        create_role("tx-role", &policy(&[(&docs, &[CAP_FILES_READ])])).unwrap();
        let first = update_role_room_paths("tx-role", &docs, |stored| {
            assert_eq!(*stored, RoomPathScope::default());
            Ok(RoomPathScope { download: vec!["reports".into()], ..Default::default() })
        })
        .unwrap();
        assert_eq!(first.room_paths[&docs].download, ["reports"]);
        let refused = update_role_room_paths("tx-role", &docs, |stored| {
            assert_eq!(stored.download, ["reports"], "the rule sees the stored line");
            Err(PathsUpdateError::Refused("private".into()))
        });
        assert_eq!(refused.unwrap_err(), PathsUpdateError::Refused("private".into()));
        assert_eq!(find_role("tx-role").unwrap().unwrap().room_paths[&docs].download, ["reports"], "nothing written");
        let bad = update_role_room_paths("tx-role", &uuid::Uuid::new_v4().to_string(), |_| Ok(RoomPathScope::default()));
        assert!(matches!(bad, Err(PathsUpdateError::Bad(ref e)) if e.contains("no line for that room")), "{bad:?}");
    }

    /// RS-A3 fail closed: a stored scope that doesn't parse, or a session
    /// with no principal, is unreadable (callers answer the missing-cap
    /// 403), never "no folders, whole room".
    #[test]
    fn an_unreadable_scope_fails_closed() {
        use super::super::*;
        let _home = crate::test_env::TempHome::new();
        let docs = uuid::Uuid::new_v4().to_string();
        let role = create_role("broken", &policy(&[(&docs, &[CAP_FILES_READ])])).unwrap();
        add_principal("bea").unwrap();
        assign_role("bea", "broken").unwrap();
        let (_, raw) = create_session_token("bea").unwrap();
        with_conn(|c| {
            c.execute("UPDATE roles SET room_paths = 'not json' WHERE id = ?1", params![role.id])
                .map_err(|e| e.to_string())
        })
        .unwrap();
        let pass = resolve_skin_token(&raw).expect("the login itself still resolves");
        assert!(matches!(pass.room_paths, PassPaths::Unreadable(_)), "{:?}", pass.room_paths);
        assert!(pass.path_scope(&docs).is_err(), "corrupt scope must not read as the whole room");
        // A session pass with no principal can't name a grant: unreadable too.
        let mut ghost = pass.clone();
        ghost.principal_id = None;
        let ghost_paths = with_conn(|c| Ok(session_paths(c, &ghost))).unwrap();
        assert!(matches!(ghost_paths, PassPaths::Unreadable(_)));
    }

    #[test]
    fn stored_paths_parse_strictly() {
        assert!(parse_room_paths(None).unwrap().is_empty());
        assert!(parse_room_paths(Some("{}")).unwrap().is_empty());
        let p = parse_room_paths(Some(r#"{"r1":{"read":["a/"],"uploadDir":null}}"#)).unwrap();
        assert_eq!(p["r1"].read, ["a"]);
        assert!(parse_room_paths(Some("[]")).is_err());
        assert!(parse_room_paths(Some(r#"{"r1":{"reed":["a"]}}"#)).is_err(), "unknown keys fail");
        // Drop folders parse (AC8) and are checked like any other list.
        let p = parse_room_paths(Some(r#"{"r1":{"upload":["Inbox/"]}}"#)).unwrap();
        assert_eq!(p["r1"].upload, ["Inbox"]);
        assert!(p["r1"].is_scoped());
        assert!(parse_room_paths(Some(r#"{"r1":{"upload":[".k2"]}}"#)).is_err());
        assert!(parse_room_paths(Some(r#"{"r1":{"upload":"Inbox"}}"#)).is_err(), "a list, not a string");
        assert!(parse_room_paths(Some(r#"{"r1":{"read":["../x"]}}"#)).is_err());
        assert!(parse_room_paths(Some("not json")).is_err());
    }
}
