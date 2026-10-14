//! Trigger note builder (contract §9 "Note template", <= 4 KB). Owner: B2.
//! Only the wake pipeline calls it, so B2 may reshape [`NoteInput`].

use super::extract::Projection;
use super::state::{SourceState, Transition};
use super::store::SourceRow;

/// One event in a (possibly coalesced) note.
pub struct NoteEvent<'a> {
    pub source: &'a SourceRow,
    pub transition: &'a Transition,
    pub state: &'a SourceState,
    pub projection: Option<&'a Projection>,
    pub at: i64,
}

pub struct NoteInput<'a> {
    /// Oldest first.
    pub events: Vec<NoteEvent<'a>>,
    /// The server this check runs on (`Runs on <host>`).
    pub host: &'a str,
    /// The trigger's owner note.
    pub owner_note: Option<&'a str>,
    /// A test wake from Settings (`[TEST] `).
    pub test: bool,
}

/// The note text, <= `NOTE_MAX_BYTES`.
///
/// C0 skeleton: empty (B2 replaces it).
pub fn build(input: &NoteInput) -> String {
    let _ = input;
    String::new()
}

/// The inbox fallback title: `source:<name> <phrase>`.
pub fn title(source_name: &str, phrase: &str) -> String {
    format!("source:{source_name} {phrase}")
}
