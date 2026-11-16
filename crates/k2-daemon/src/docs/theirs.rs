//! "Use their version" after an outside save of a **text** doc (the
//! editors' notice: Compare · Undo their save · Use their version).
//!
//! The kit merges an outside save into the live doc, the person's edits not
//! yet written back included, and keeps the merged doc as the "external
//! edit" version; the file as the other program saved it isn't kept
//! anywhere once the kit writes the merged doc back. So K2 keeps it: the
//! docs watcher reads the file's text just before it tells the kit
//! ([`ingest`]), and when the kit takes the save in (`Ingested`, change
//! `rev`), the text is remembered for that doc ([`remember`]), the newest
//! save only, in memory (a restart forgets it).
//!
//! `POST /cli/docs/use-theirs {path, rev?}` then makes the doc that text as
//! **one** change by the caller at the current version (the kit's replace,
//! `--base live`: it diffs, so only what differs changes), which their undo
//! reverses. `rev` (the `outside_save` frame's) must be the remembered
//! save's; anything else is 409 `theirs_gone`.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use k2_mpc_base::session::{DiskCheck, Session, SessionError};
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

/// The watcher's `file_changed` for a text doc's file at `path`: the file's
/// text is read first, and kept as the save when the kit takes it in. A
/// file that isn't UTF-8 (the pack declines it) or is too large keeps
/// nothing. A save in the instant between the read and the kit's read is
/// the next event's (the watcher sees that write too).
pub fn ingest(s: &Session, key: &DocKey, path: &Path) -> Result<DiskCheck, SessionError> {
    let before = std::fs::read(path).ok().filter(|b| b.len() <= TEXT_MAX).and_then(|b| String::from_utf8(b).ok());
    let r = s.file_changed()?;
    if let (DiskCheck::Ingested { version }, Some(text)) = (&r, before) {
        remember(key, *version, text);
    }
    Ok(r)
}

#[cfg(test)]
#[path = "theirs_tests.rs"]
mod tests;
