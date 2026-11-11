//! `.k2/agent/template.lock` (prd-agent-templates-v1 §3): what a workspace
//! got from its template. TOML, inside the workspace, so it travels with
//! clones and moves. Unknown keys are kept tolerable (a newer K2's lock
//! still reads), unlike the template itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::super::HireError;

pub const LOCK_REL: &str = ".k2/agent/template.lock";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateLock {
    pub id: String,
    pub version: u32,
    /// The template's hash when last applied.
    pub hash: String,
    pub applied_at: i64,
    /// The actor (`owner-token`, `user:<name>`, `agent:<ws>`, `api-key:<id>`).
    pub by: String,
    #[serde(default)]
    pub catalog: Vec<String>,
    /// What the template suggested (sets expanded).
    #[serde(default)]
    pub suggested: Vec<String>,
    /// Switches the permissions step turned on (T3).
    #[serde(default)]
    pub granted: Vec<String>,
    /// Suggestions filed as cards for a person (T3).
    #[serde(default)]
    pub carded: Vec<String>,
    /// Workspace-relative path → sha256 of the text as written. For
    /// `.k2/agent/ROLE.md` it is the K2:MANAGED block's body. The upgrade
    /// (T5) replaces a file only while its sha still matches.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

pub fn lock_path(workspace: &str) -> PathBuf {
    Path::new(workspace).join(LOCK_REL)
}

/// The lock, if the workspace has one. A lock that doesn't parse is an
/// error (never silently treated as absent: that would let a second
/// template in).
pub fn read(workspace: &str) -> Result<Option<TemplateLock>, HireError> {
    let p = lock_path(workspace);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(HireError::internal(format!("{}: {e}", p.display()))),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| HireError::new(409, "bad_template_lock", format!("{} doesn't parse ({e}); fix or remove it, then re-run", p.display())))
}

pub fn write(workspace: &str, lock: &TemplateLock) -> Result<(), HireError> {
    let p = lock_path(workspace);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HireError::internal(format!("{}: {e}", parent.display())))?;
    }
    let mut text = String::from("# Written by K2 when this agent was made from a template. Don't edit.\n");
    text.push_str(&toml::to_string(lock).map_err(|e| HireError::internal(format!("template lock: {e}")))?);
    crate::fs_atomic::atomic_write_str(&p, &text).map_err(|e| HireError::internal(format!("{}: {e}", p.display())))
}

impl TemplateLock {
    /// Equal ignoring when and by whom (a re-apply that changes nothing
    /// else leaves the file alone).
    pub fn same_content(&self, other: &TemplateLock) -> bool {
        let strip = |l: &TemplateLock| TemplateLock { applied_at: 0, by: String::new(), ..l.clone() };
        strip(self) == strip(other)
    }
}
