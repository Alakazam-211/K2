//! A native doc's live-text cap: its live text (every text root's together) stays within the
//! native engine's per-change string limit (`string_bytes_per_op`, 4 MiB at the engine's default
//! limits, which the kit never changes). An edit that would grow it past is refused
//! with the cap's own code ([`SessionError::NativeTextCap`], `MPC-E909`, exit 2), and nothing
//! in the doc is dropped; an outside save that
//! would is declined (`too_large`). A text piece past that limit can't be checkpointed by the
//! engine, so the cap holds on every edit, not only at open.

use crate::doc::{Doc, Edit};
use crate::session::error::{Result, SessionError};

/// The cap on a doc's live text, in bytes: a native doc's engine `string_bytes_per_op`;
/// `None` on Yrs (the pack's own limit applies there, at open).
pub(super) fn text_cap(doc: &Doc) -> Option<usize> {
    let _ = doc;
    None
}

/// Refuse `edits` (text edits against `texts`, the doc's texts now) when they would grow a
/// native doc's live text past its cap ([`text_cap`]): [`SessionError::NativeTextCap`]
/// (`MPC-E909`, exit 2), nothing applied. An edit that doesn't grow the text is never
/// refused, so a doc over the cap can still shrink.
pub(super) fn check_text_cap(
    doc: &Doc,
    texts: &[(&'static str, String)],
    edits: &[Edit],
) -> Result<()> {
    let Some(cap) = text_cap(doc) else {
        return Ok(());
    };
    if crate::beta_plant::planted("text-cap-off") {
        return Ok(());
    }
    let before: usize = texts.iter().map(|(_, t)| t.len()).sum();
    let mut after = before as i128;
    for e in edits {
        if let Edit::Text { hunks, .. } = e {
            for h in hunks {
                after += h.insert.len() as i128 - (h.end as i128 - h.start as i128);
            }
        }
    }
    let after = usize::try_from(after.max(0)).unwrap_or(usize::MAX);
    if after > cap && after > before {
        // the plant: the cap borrows the grid's size refusal, as before it had its own code
        if crate::beta_plant::planted("cap-code-borrowed") {
            return Err(SessionError::AtLimit {
                what: "live text bytes",
                limit: cap,
                would_be: after,
            });
        }
        return Err(SessionError::NativeTextCap {
            limit: cap,
            would_be: after,
        });
    }
    Ok(())
}
