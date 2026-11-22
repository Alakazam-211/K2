//! `.k2/agent/template.lock` (prd-agent-templates-v1 §3): what a workspace
//! got from its template. TOML, inside the workspace, so it travels with
//! clones and moves. Unknown keys are kept tolerable (a newer K2's lock
//! still reads), unlike the template itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::super::HireError;
use crate::product_name;

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
    /// Files the agent had changed when an upgrade came: kept, with the new
    /// text beside each as `<file>.proposed` (T5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    /// Workspace-relative path → sha256 of the text as written. For
    /// `.k2/agent/ROLE.md` it is the K2:MANAGED block's body; a static
    /// catalog pack the template stacked is here too (AT10). The upgrade
    /// (T5) replaces a file only while its sha still matches.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// The values the template's text was rendered with (`name`, `person`,
    /// the params, `open_questions`), so an upgrade renders the next
    /// version the same way. Empty in a lock written before upgrades.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    /// The hire's answers to the template's questions (they are in ROLE.md
    /// or a starter note already).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub answers: BTreeMap<String, String>,
    /// The hello still to send (rendered at the first create): whatever
    /// starts the agent first sends it once, then clears it (T9). Absent
    /// in a lock written before this field, and once sent: an agent made
    /// before it never gets a surprise hello.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hello_pending: Option<String>,
    /// When the hello was taken to be sent (unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hello_sent_at: Option<i64>,
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
    let mut text = String::from(concat!("# Written by ", product_name!(), " when this agent was made from a template. Don't edit.\n"));
    text.push_str(&toml::to_string(lock).map_err(|e| HireError::internal(format!("template lock: {e}")))?);
    crate::fs_atomic::atomic_write_str(&p, &text).map_err(|e| HireError::internal(format!("{}: {e}", p.display())))
}

/// One take of a pending hello at a time in this process, so two starts
/// of one agent never both send it.
static HELLO_TAKE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The workspace's lock holds a hello still to send (a cheap read; it
/// takes nothing).
pub fn has_pending_hello(workspace: &str) -> bool {
    matches!(read(workspace), Ok(Some(l)) if l.hello_pending.as_deref().is_some_and(|h| !h.trim().is_empty()))
}

/// Take the pending hello: clear it and record `hello_sent_at` in the lock
/// FIRST, then return its text for the caller to deliver. `None` when
/// there is no lock, the lock predates the field, or it went already.
pub fn take_pending_hello(workspace: &str) -> Result<Option<String>, HireError> {
    let _g = HELLO_TAKE.lock().unwrap_or_else(|p| p.into_inner());
    let Some(mut l) = read(workspace)? else { return Ok(None) };
    let Some(text) = l.hello_pending.take().filter(|h| !h.trim().is_empty()) else { return Ok(None) };
    l.hello_sent_at = Some(super::super::now_secs());
    write(workspace, &l)?;
    Ok(Some(text))
}

impl TemplateLock {
    /// Equal ignoring when and by whom (a re-apply that changes nothing
    /// else leaves the file alone).
    pub fn same_content(&self, other: &TemplateLock) -> bool {
        let strip = |l: &TemplateLock| TemplateLock { applied_at: 0, by: String::new(), ..l.clone() };
        strip(self) == strip(other)
    }
}
