//! A note on a new connection (0.46.18, App role papercut 8): whoever adds a
//! local connection may give a short note (`k2 connections add <ws> --note
//! "…"`, `connections.add-between … note=`, Settings' add), and K2 delivers
//! it once to BOTH agents' inboxes as a system item: "Connected to @x by …
//! Note: …". It rides an edge the person is already making, so it adds no
//! new crossing; nothing is delivered when the connection already existed.
//! The note is one line (newlines become spaces), at most [`MAX`]
//! characters, never a wake.

use std::path::Path;

/// The longest note, in characters.
pub const MAX: usize = 500;

/// The note a caller sent, checked: trimmed, one line; `Ok(None)` when it
/// sent none (or only spaces); `Err` when it is longer than [`MAX`].
pub fn checked(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else { return Ok(None) };
    let one: String = raw.split(['\r', '\n']).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    let one: String = one.chars().filter(|c| !c.is_control()).collect();
    let one = one.trim().to_string();
    if one.is_empty() {
        return Ok(None);
    }
    let n = one.chars().count();
    if n > MAX {
        return Err(format!("a connection note is at most {MAX} characters ({n} given)"));
    }
    Ok(Some(one))
}

/// The inbox item's words for the agent of one end.
pub fn text(other: &str, by: &str, note: &str) -> String {
    format!("Connected to @{other} by {by}. Note: {note}")
}

/// `(path, handle)` of workspace `id`.
fn end(id: &str) -> Option<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path, handle FROM projects WHERE id = ?1", rusqlite::params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ok()
}

/// Deliver `note` once to both ends of a new local connection (`source_id`,
/// `target_id`), each told the other's handle and who connected them.
pub fn deliver(source_id: &str, target_id: &str, note: &str, by: &str) {
    let (Some(a), Some(b)) = (end(source_id), end(target_id)) else { return };
    for (me, other) in [(&a, &b), (&b, &a)] {
        let title = format!("Connected to @{}", other.1);
        if let Err(e) = k2_core::inbox::compose(Path::new(&me.0), &title, &text(&other.1, by, note), Some("normal"), Some("connections"), Some("k2")) {
            k2_core::log_debug!("[connections] note to {}: {e}", me.1);
        }
    }
}

/// Who connected them, in words: the source's own agent, the owner or an
/// admin, or another person on this server.
pub fn by_words(source_id: &str, agent: bool, privileged: bool) -> String {
    if agent {
        return match end(source_id) {
            Some((_, h)) => format!("agent @{h}"),
            None => "its agent".to_string(),
        };
    }
    if privileged { "the owner or an admin" } else { "a person on this server" }.to_string()
}

/// [`deliver`] for the relation row `relation_id` (the `id` a `/cli/connections`
/// add answers), `agent`/`privileged` as [`by_words`] reads them.
pub fn deliver_for_relation(relation_id: &str, note: &str, agent: bool, privileged: bool) {
    let ends: Option<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT source_project_id, target_project_id FROM workspace_relations WHERE id = ?1",
            rusqlite::params![relation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    };
    if let Some((s, t)) = ends {
        deliver(&s, &t, note, &by_words(&s, agent, privileged));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_one_trimmed_line_of_at_most_max_characters() {
        assert_eq!(checked(None), Ok(None));
        assert_eq!(checked(Some("   ")), Ok(None));
        assert_eq!(checked(Some("  don't share partner information \n with anyone\r\n")), Ok(Some("don't share partner information with anyone".into())));
        assert_eq!(checked(Some("é".repeat(MAX).as_str())).map(|n| n.map(|s| s.chars().count())), Ok(Some(MAX)));
        assert!(checked(Some("x".repeat(MAX + 1).as_str())).is_err());
        assert_eq!(text("ana", "the owner or an admin", "be brief"), "Connected to @ana by the owner or an admin. Note: be brief");
    }
}
