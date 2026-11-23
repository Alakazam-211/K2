//! Owner-approved wiki locations.
//!
//! A workspace's `.k2/wiki` (or its `.k2`) may be a link to a folder
//! elsewhere, an Obsidian vault say. Owner and agent writes in the
//! workspace follow it as they always have. API-key and template writes go
//! through it only when its resolved target is the location the owner
//! approved for that workspace: the owner approves once (`k2 wiki location
//! --approve`, `POST /cli/wiki/location/approve`), the approval records the
//! resolved target, and a link that now resolves elsewhere needs a new
//! approval.
//!
//! The approvals live outside every workspace, in
//! `~/.k2/wiki-locations.json` (0600):
//! `{"version":1,"approved":{"<workspace real path>":{"target":"<real target>","approvedAt":"<RFC3339>"}}}`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// `~/.k2/wiki-locations.json`.
pub fn store_path() -> PathBuf {
    crate::paths::k2_home().join("wiki-locations.json")
}

/// One approval: the resolved target the owner approved, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub target: String,
    pub approved_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    approved: BTreeMap<String, Approval>,
}

/// The store; empty when there is no file. A file that can't be read or
/// parsed is an error (a write that asks for an approval is refused, never
/// let through).
fn load() -> Result<Store, String> {
    let p = store_path();
    match fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn save(store: &Store) -> Result<(), String> {
    let p = store_path();
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_private(&p, &bytes).map_err(|e| format!("{}: {e}", p.display()))
}

/// A link: a symlink, or on Windows any reparse point (a junction too).
pub(super) fn is_link(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT.
        if m.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    m.file_type().is_symlink()
}

/// `.k2` or `.k2/wiki` under the workspace's real path `root` is a link.
pub fn is_linked(root: &Path) -> bool {
    let mut cur = root.to_path_buf();
    Path::new(super::WIKI_REL).components().any(|part| {
        cur.push(part);
        fs::symlink_metadata(&cur).is_ok_and(|m| is_link(&m))
    })
}

/// Where a workspace's wiki stands for API-key and template writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationState {
    /// The workspace's real path.
    pub workspace: String,
    pub linked: bool,
    /// The wiki's resolved target when it is a link that resolves.
    pub target: Option<String>,
    pub approved: Option<Approval>,
    /// `not_linked`, `approved`, `not_approved`, `changed` (approved for
    /// another target) or `missing_target` (a link to nothing).
    pub status: &'static str,
}

impl LocationState {
    /// Whether an API key or a template may write through this wiki.
    pub fn outside_writes_ok(&self) -> bool {
        matches!(self.status, "not_linked" | "approved")
    }

    /// One line for a person.
    pub fn line(&self) -> String {
        let ws = &self.workspace;
        let target = self.target.as_deref().unwrap_or("?");
        match self.status {
            "not_linked" => format!("{ws}/.k2/wiki is a folder in the workspace; there is nothing to approve."),
            "approved" => format!(
                "{ws}/.k2/wiki is a link to {target}, approved {}: API keys and templates may add notes there.",
                self.approved.as_ref().map_or("", |a| a.approved_at.as_str())
            ),
            "changed" => format!(
                "{ws}/.k2/wiki is a link to {target}, but the approved location is {}: API keys and templates can't add notes until the owner approves the new one ({} wiki location --approve).",
                self.approved.as_ref().map_or("?", |a| a.target.as_str()),
                crate::brand::CLI_NAME
            ),
            "missing_target" => format!("{ws}/.k2/wiki is a link to a folder that isn't there."),
            _ => format!(
                "{ws}/.k2/wiki is a link to {target}, not approved: API keys and templates can't add notes there until the owner approves it ({} wiki location --approve, or Settings).",
                crate::brand::CLI_NAME
            ),
        }
    }
}

/// The state of the workspace at real path `root`.
fn state_of(root: &Path, store: &Store) -> LocationState {
    let workspace = root.to_string_lossy().into_owned();
    let approved = store.approved.get(&workspace).cloned();
    if !is_linked(root) {
        return LocationState { workspace, linked: false, target: None, approved, status: "not_linked" };
    }
    let target = dunce::canonicalize(root.join(super::WIKI_REL))
        .ok()
        .filter(|t| t.is_dir())
        .map(|t| t.to_string_lossy().into_owned());
    let status = match (&target, &approved) {
        (None, _) => "missing_target",
        (Some(t), Some(a)) if *t == a.target => "approved",
        (Some(_), Some(_)) => "changed",
        (Some(_), None) => "not_approved",
    };
    LocationState { workspace, linked: true, target, approved, status }
}

fn real(workspace: &Path) -> Result<PathBuf, String> {
    dunce::canonicalize(workspace).map_err(|e| format!("workspace path {}: {e}", workspace.display()))
}

/// Where the workspace's wiki stands.
pub fn state(workspace: &Path) -> Result<LocationState, String> {
    Ok(state_of(&real(workspace)?, &load()?))
}

/// The owner approves the wiki's current resolved target for this
/// workspace. Only a linked wiki whose target exists can be approved.
pub fn approve(workspace: &Path) -> Result<LocationState, String> {
    let root = real(workspace)?;
    let mut store = load()?;
    let st = state_of(&root, &store);
    let target = match (st.status, st.target) {
        ("not_linked", _) => return Err(st_line_not_linked(&root)),
        (_, None) => return Err(format!("{}/.k2/wiki is a link to a folder that isn't there; nothing to approve", root.display())),
        (_, Some(t)) => t,
    };
    store.version = 1;
    store.approved.insert(
        root.to_string_lossy().into_owned(),
        Approval { target, approved_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true) },
    );
    save(&store)?;
    Ok(state_of(&root, &store))
}

fn st_line_not_linked(root: &Path) -> String {
    format!("{}/.k2/wiki is not a link; there is nothing to approve", root.display())
}

/// For an API-key or template write into the workspace at real path
/// `root`: `Ok(Some(target))`, the approved resolved target to write in,
/// when the wiki is a link; `Ok(None)` when it is not a link (write from
/// `root` without following one); `Err` when the link isn't approved.
pub(super) fn outside_writer_target(root: &Path) -> Result<Option<PathBuf>, String> {
    let st = state_of(root, &load()?);
    if !st.outside_writes_ok() {
        return Err(st.line());
    }
    Ok(st.target.map(PathBuf::from))
}

/// The doctor's lines: each of `workspaces` whose wiki is a link not
/// approved (or approved for another target).
pub fn doctor_lines(workspaces: &[PathBuf]) -> Vec<String> {
    let Ok(store) = load() else {
        return vec![format!("{} can't be read; no linked wiki counts as approved", store_path().display())];
    };
    workspaces
        .iter()
        .filter_map(|w| dunce::canonicalize(w).ok())
        .map(|root| state_of(&root, &store))
        .filter(|st| matches!(st.status, "not_approved" | "changed"))
        .map(|st| {
            format!(
                "{}: the wiki is a link to {}, not approved for API keys and templates ({} wiki location --approve {})",
                st.workspace,
                st.target.as_deref().unwrap_or("?"),
                crate::brand::CLI_NAME,
                st.workspace
            )
        })
        .collect()
}
