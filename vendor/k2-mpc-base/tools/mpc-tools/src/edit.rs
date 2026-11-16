//! `edit`: small exact edits of a text file.
//!
//! `edit replace-once FILE FIND REPLACE`: replace FIND with REPLACE in FILE when FIND occurs
//! exactly once (counted without overlaps); otherwise refuse (exit 3) and leave the file as it
//! is. Bytes other than the match are kept as they are (no newline translation).

use std::path::PathBuf;

use serde_json::json;

use crate::{Ctx, Fail, Report, OK};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Replace a text that occurs exactly once, or refuse
    ReplaceOnce {
        /// The file to edit (UTF-8)
        file: PathBuf,
        /// The exact text to find
        #[arg(allow_hyphen_values = true)]
        find: String,
        /// What replaces it
        #[arg(allow_hyphen_values = true)]
        replace: String,
    },
}

pub fn run(cmd: &Cmd, ctx: &Ctx) -> Result<Report, Fail> {
    match cmd {
        Cmd::ReplaceOnce {
            file,
            find,
            replace,
        } => {
            let p = ctx.path(file);
            let bytes = std::fs::read(&p).map_err(|e| Fail::io(&p, e))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| Fail::error(format!("replace-once: {}: not UTF-8", file.display())))?;
            let n = text.matches(find.as_str()).count();
            if n != 1 {
                return Err(Fail::refused(format!(
                    "replace-once: {}: the text to find occurs {n} times, not once; nothing changed",
                    file.display()
                )));
            }
            std::fs::write(&p, text.replace(find.as_str(), replace))
                .map_err(|e| Fail::io(&p, e))?;
            Ok(Report::new(
                OK,
                String::new(),
                json!({ "file": file, "replaced": 1 }),
            ))
        }
    }
}
