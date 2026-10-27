//! Markdown file codec: the kit's plain-text codec with Markdown's limits.

use k2_mpc_base::codec::text_file::{self, Decline, FileFormat};

use crate::MAX_FILE_BYTES;

/// File bytes -> doc text, plus the format to write them back. Declined files open the old way.
pub fn decode(bytes: &[u8]) -> Result<(String, FileFormat), Decline> {
    text_file::decode(bytes, MAX_FILE_BYTES)
}

/// Doc text -> file bytes in the file's own format.
pub fn encode(text: &str, fmt: FileFormat) -> Vec<u8> {
    text_file::encode(text, fmt)
}

/// Normalise incoming text (editor, agent, disk) so the doc never holds `\r`.
pub use text_file::normalize_input;
