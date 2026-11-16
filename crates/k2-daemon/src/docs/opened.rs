//! The version an app opened (kit v0.21.0, the outside-save base bridge).
//!
//! When K2 opens a doc's file in another app (an editor:
//! `/cli/projects/open-in-editor` with a file's path; `/cli/fs/open-external`
//! opens links only, never a path), it first tells the kit
//! (`note_opened(conn, OpenedAt::FileNow)`). The kit takes in an outside
//! save it hasn't seen, writes back what the file doesn't hold yet, pins the
//! version the file holds and holds its write-backs to the file for
//! `Config::opened_hold_ms` (the kit's default, 10 s), so the app reads that
//! version. That app's next save then merges from the version it opened, not
//! from whatever the doc holds by then: live edits made meanwhile are kept,
//! never overwritten by the app's older copy.
//!
//! - **Loaded docs only.** A doc that isn't loaded has nothing unwritten,
//!   and loading one just for this would load a doc for every file anyone
//!   opens. The participant is the box owner's (these routes open apps on
//!   this machine).
//! - **No clear on close.** The OS opens the app and returns, so K2 never
//!   sees it close and doesn't call `clear_opened`. The kit moves the pin
//!   at the app's first save, drops it when its version is gone, and the
//!   next open replaces it.
//! - **Never in the way.** A refusal (MPC-E180 to MPC-E182) is logged and
//!   the app opens as it always did; its save then merges as before.
//! - **A version that is gone (MPC-E181) is never asked for again:** [`note`]
//!   notes the file as it is now instead.

use std::path::{Path, PathBuf};

use k2_core::log_debug;
use k2_mpc_base::session::{BlobSession, Opened, OpenedAt, Participant, Session, SessionError, Stay};

/// How the owner's connection reached the doc (logged per change as `via`;
/// noting makes no change).
pub const VIA_OPEN_IN_APP: &str = "open-in-app";

/// Note `at` through `f` (a session's `note_opened` on a joined
/// connection). A version that is gone (MPC-E181) is never retried: the
/// file as it is now is noted instead, once.
pub fn note<T>(at: OpenedAt, mut f: impl FnMut(OpenedAt) -> Result<T, SessionError>) -> Result<T, SessionError> {
    match f(at) {
        Err(SessionError::OpenedGone { .. }) if at != OpenedAt::FileNow => f(OpenedAt::FileNow),
        r => r,
    }
}

/// A live doc's session: `who` joins (a one-shot connection) and notes `at`.
pub fn note_live(s: &Session, who: Participant, at: OpenedAt) -> Result<Opened, SessionError> {
    let j = s.join_shared(who, Stay::UntilIdle)?;
    note(at, |at| s.note_opened(j.conn, at))
}

/// An any-file doc's session: `who` joins and notes `at`.
pub fn note_blob(s: &BlobSession, who: Participant, at: OpenedAt) -> Result<Opened, SessionError> {
    let j = s.join_shared(who, Stay::UntilIdle)?;
    note(at, |at| s.note_opened(j.conn, at))
}

/// The box owner, as the participant that notes an open.
fn owner() -> Participant {
    super::caller::DocCaller::Person { key: k2_core::notify_prefs::OWNER.to_string(), via: VIA_OPEN_IN_APP }.participant(None)
}

/// The paths a loaded doc's file may be known by: as given, and resolved.
fn candidates(path: &Path) -> Vec<PathBuf> {
    let mut v = vec![path.to_path_buf()];
    if let Ok(c) = path.canonicalize() {
        if c != path {
            v.push(c);
        }
    }
    v
}

/// Right before K2 opens `path` in another app: every loaded doc whose file
/// it is notes the file as it is now. Returns how many were noted. Never
/// fails the open: no docs runtime, no loaded doc, or a refusal is a no-op
/// (a refusal is logged).
pub fn before_app_opens(path: &Path) -> usize {
    let Ok(rt) = super::runtime() else { return 0 };
    let mut noted = 0;
    let mut seen = Vec::new();
    for p in candidates(path) {
        for (key, is_file) in rt.docs_at(&p) {
            if !is_file || seen.contains(&key) {
                continue;
            }
            seen.push(key.clone());
            let r = if let Some(s) = rt.get(&key) {
                note_live(&s, owner(), OpenedAt::FileNow)
            } else if let Some(s) = rt.get_blob(&key) {
                note_blob(&s, owner(), OpenedAt::FileNow)
            } else {
                continue;
            };
            match r {
                Ok(o) => {
                    noted += 1;
                    log_debug!("[docs] {} opened in another app at version {}", super::host::key_label(&key), o.version)
                }
                Err(e) => log_debug!(
                    "[docs] WARN: {} opened in another app, not noted ({}): its next save merges from the latest version",
                    super::host::key_label(&key),
                    e.report().id()
                ),
            }
        }
    }
    noted
}

#[cfg(test)]
#[path = "opened_tests.rs"]
mod tests;
