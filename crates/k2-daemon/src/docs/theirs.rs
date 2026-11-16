//! "Use their version" after an outside save of a **text** doc (the
//! editors' notice: Compare · Undo their save · Use their version).
//!
//! The kit merges an outside save into the live doc, the person's edits not
//! yet written back included, and keeps the merged doc as the "external
//! edit" version; the file as the other program saved it isn't kept
//! anywhere once the kit writes the merged doc back. So K2 keeps it: when
//! the kit takes a save in (its audit event `ExternalEdit`, change `rev`,
//! under the doc's lock, before any write-back), K2 reads the file's text
//! and remembers it for that doc ([`on_external_edit`]), the newest save
//! only, in memory (a restart forgets it).
//!
//! `POST /cli/docs/use-theirs {path, rev?}` then makes the doc that text as
//! **one** change by the caller at the current version (the kit's replace,
//! `--base live`: it diffs, so only what differs changes), which their undo
//! reverses. `rev` (the `outside_save` frame's) must be the remembered
//! save's; anything else is 409 `theirs_gone`.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use k2_mpc_base::store::DocKey;

/// Docs whose last outside save is kept (the oldest goes first).
pub const KEPT: usize = 32;

/// The largest text kept (the text pack's own cap for a live text doc).
pub const TEXT_MAX: usize = 4 << 20;

/// One doc's last outside save as it was on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theirs {
    /// The save's change (the `outside_save` frame's `rev`).
    pub rev: u64,
    pub text: String,
}

fn kept() -> &'static Mutex<VecDeque<(DocKey, Theirs)>> {
    static K: OnceLock<Mutex<VecDeque<(DocKey, Theirs)>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Keep `text` as `key`'s last outside save, at change `rev`.
pub fn remember(key: &DocKey, rev: u64, text: String) {
    if text.len() > TEXT_MAX {
        return;
    }
    let mut k = kept().lock().unwrap_or_else(|p| p.into_inner());
    k.retain(|(d, _)| d != key);
    if k.len() >= KEPT {
        k.pop_front();
    }
    k.push_back((key.clone(), Theirs { rev, text }));
}

/// `key`'s last outside save, if K2 still has it.
pub fn get(key: &DocKey) -> Option<Theirs> {
    kept().lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(d, _)| d == key).map(|(_, t)| t.clone())
}

/// The doc was renamed or purged: what was kept under its old key goes.
pub fn forget(key: &DocKey) {
    kept().lock().unwrap_or_else(|p| p.into_inner()).retain(|(d, _)| d != key);
}

/// The kit took an outside save of `key`'s file in, as change `rev`
/// (`AuditEvent::ExternalEdit`, from the host's audit hook). The kit calls
/// the hook under the doc's lock, right after it read the file and before
/// it can write anything back, so the file still holds the save as the
/// other program wrote it: read it now and keep it. A text doc only, in a
/// registered workspace; a file that isn't UTF-8 or is too large keeps
/// nothing.
pub fn on_external_edit(key: &DocKey, rev: u64) {
    if !super::host_events::is_text_doc(key) {
        return;
    }
    // A read-pool read: it never waits behind a writer while the doc is locked.
    let root = k2_core::db::read(|conn| k2_core::db::schema::Project::get(conn, &key.space).ok().map(|p| p.path));
    let Some(root) = root else { return };
    if let Some(text) = read_text(&Path::new(&root).join(&key.path)) {
        remember(key, rev, text);
    }
}

/// A file's text, when it is UTF-8 and at most [`TEXT_MAX`] bytes.
pub fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > TEXT_MAX as u64 {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
#[path = "theirs_tests.rs"]
mod tests;
