//! Files (S25 §9). S25.1 ships the signed-URL download primitive; the resumable
//! `download_attachment_to` and the three-step upload helper land in S25.3 with S15.

mod calls;
mod download;

pub use calls::UploadRange;
pub use download::{ByteRange, ContentRange, Download};
