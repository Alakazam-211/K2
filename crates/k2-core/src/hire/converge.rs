//! Create-or-converge ONE workspace at a path (`POST /v1/w`,
//! `k2 agent hire <dir>` via `POST /cli/agents/hire`). Re-running the same
//! request changes nothing and reports `changed: false`. No undo: converge
//! heals missing pieces, it never takes a workspace down.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::recipe::{InlineLayer, WikiNote};
use super::steps::{self, PersonaSource};
use super::{HireCaller, HireError};

/// The converge request (the `/v1/w` body; the CLI sends the same shape).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvergeRequest {
    /// The folder. Required here; a template hire (`apply`) may leave it
    /// empty for `<hire root>/<handle>`.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    /// An agent template id or alias (`writer`, `worker`): resolved through
    /// the template registry (prd-agent-templates-v1 AT11).
    #[serde(default)]
    pub template: Option<String>,
    /// The template's `[params]` (`about`), and `person`.
    #[serde(default)]
    pub params: Option<std::collections::BTreeMap<String, String>>,
    /// Answers to the template's `[[questions]]`, by question id
    /// (`k2 agent template show <id>` lists them). The agent asks its
    /// person the rest on its first start.
    #[serde(default)]
    pub answers: Option<std::collections::BTreeMap<String, String>>,
    /// ROLE.md body.
    #[serde(default)]
    pub persona: Option<String>,
    /// A whole ROLE.md (frontmatter included) — `k2 agent hire --persona`.
    #[serde(default)]
    pub persona_raw: Option<String>,
    /// Role for the generated frontmatter (CLI `--template` / scaffold).
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub wiki: Option<Vec<WikiNote>>,
    #[serde(default)]
    pub context: Option<Vec<String>>,
    #[serde(default)]
    pub layers: Option<Vec<InlineLayer>>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub no_wiki: Option<bool>,
    #[serde(default)]
    pub no_agents_md: Option<bool>,
    #[serde(default)]
    pub fanout: Option<bool>,
    /// Persist create-only `db_agent_access` (`off`/`read`/`write`). Does
    /// NOT mint a database.
    #[serde(default)]
    pub db_access: Option<String>,
    /// A template hire: suggested permissions the person unticked on the
    /// Review step (action ids, or `permission-cards`). Absent: every
    /// suggestion is decided by the hirer's reach (prd-agent-templates-v1
    /// §6 step 4, T3).
    #[serde(default)]
    pub decline_permissions: Option<Vec<String>>,
    /// A re-apply by a person: suggestions declined at an earlier hire that
    /// they name again (`--allow`). Without it a decline holds.
    #[serde(default)]
    pub allow_permissions: Option<Vec<String>>,
    /// The door grants its own starter set (the first-run routes). Never
    /// read from a request body.
    #[serde(skip)]
    pub permissions_by_door: bool,
}

/// What the caller may do beyond the shape of the request.
#[derive(Debug, Clone, Default)]
pub struct ConvergeAuthority {
    /// `Some(dir)`: a NEW workspace folder must sit inside `dir` (API keys,
    /// T2/T19). `None`: any absolute path (the owner on their own box).
    pub new_workspace_root: Option<PathBuf>,
    /// May this caller create a new workspace at all?
    pub may_create: bool,
    /// Highest `db_agent_access` this caller may set (`None` = no cap).
    pub max_db: Option<&'static str>,
    /// Refusal text when `may_create` is false.
    pub create_refusal: Option<String>,
    /// The hire only ever creates (an agent passport): the folder must not
    /// exist yet, in any spelling, and must not be a registered workspace.
    /// See [`create_only_path`] and [`claim_new_folder`].
    pub create_only: bool,
    /// An API key's template hire must make a NEW workspace: the folder may
    /// not exist (or must be empty and unregistered), so a key can't apply a
    /// template (and its permissions) to an existing workspace. Re-applying
    /// stays a person's. An agent's hire is [`Self::create_only`].
    pub new_only: bool,
}

impl ConvergeAuthority {
    pub fn unrestricted() -> Self {
        Self { new_workspace_root: None, may_create: true, max_db: None, create_refusal: None, create_only: false, new_only: false }
    }

    /// An agent passport's hire: creates a new workspace inside the hire
    /// root, never converges one that is already there.
    pub fn create_only() -> Result<Self, HireError> {
        Ok(Self { create_only: true, new_workspace_root: Some(super::root::hire_root()?), ..Self::unrestricted() })
    }
}

/// [`ConvergeAuthority::new_only`]: refuse a folder that is a registered
/// workspace, or exists with anything in it.
pub fn check_new_folder(caller: &HireCaller, path: &str) -> Result<(), HireError> {
    let who = if caller.is_agent() { "an agent" } else { "an API key" };
    if lookup_project_row(path).is_some() {
        return Err(HireError::new(
            403,
            "existing_workspace",
            format!("{path} is already a workspace: {who} hires only into a new folder. Re-applying a template to an existing agent is a person's (k2 agent hire <dir> --template <id> from their own terminal)"),
        ));
    }
    let p = Path::new(path);
    if p.exists() {
        let empty = p.is_dir() && fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !empty {
            return Err(HireError::new(
                403,
                "existing_workspace",
                format!("{path} already exists: {who} hires only into a new, empty folder (leave out <dir> for <hire root>/<handle>)"),
            ));
        }
    }
    Ok(())
}

/// One reported step (`{step, status, reason?}`); statuses are `done`,
/// `skipped`, `pending` (dry run).
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct StepReport {
    pub step: &'static str,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn done(step: &'static str) -> StepReport {
    StepReport { step, status: "done", reason: None }
}
fn skipped(step: &'static str, why: &str) -> StepReport {
    StepReport { step, status: "skipped", reason: Some(why.to_string()) }
}

#[derive(Debug, Clone)]
pub struct ConvergeOutcome {
    pub path: String,
    pub id: String,
    pub handle: String,
    pub name: String,
    pub changed: bool,
    /// The workspace was registered by this call.
    pub created: bool,
    pub steps: Vec<StepReport>,
    pub wiki_notes: Vec<serde_json::Value>,
    /// The template's own steps, when the request named one.
    pub template: Option<super::template::WorkspaceOutcome>,
}

/// Trim, expand `~`, require absolute.
fn expand_tilde(raw: &str) -> Result<PathBuf, HireError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(HireError::usage("missing path"));
    }
    let expanded = if t == "~" {
        crate::paths::home_dir().ok_or_else(|| HireError::usage("cannot resolve ~"))?
    } else if let Some(rest) = t.strip_prefix("~/") {
        crate::paths::home_dir().ok_or_else(|| HireError::usage("cannot resolve ~"))?.join(rest)
    } else {
        PathBuf::from(t)
    };
    if !expanded.is_absolute() {
        return Err(HireError::usage("path must be absolute or ~"));
    }
    Ok(expanded)
}

/// Expand `~`, require absolute, canonicalize an existing directory.
pub fn expand_hire_path(raw: &str) -> Result<String, HireError> {
    let expanded = expand_tilde(raw)?;
    if expanded.exists() {
        if !expanded.is_dir() {
            return Err(HireError::usage(format!("{} exists and is not a directory", expanded.display())));
        }
        return dunce::canonicalize(&expanded)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| HireError::usage(format!("canonicalize: {e}")));
    }
    Ok(expanded.to_string_lossy().into_owned())
}

/// A registered workspace row (id, name, handle).
#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
}

fn lookup_exact(path: &str) -> Option<ProjectRow> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, name, handle FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| Ok(ProjectRow { id: r.get(0)?, name: r.get(1)?, handle: r.get(2)? }),
    )
    .ok()
}

fn path_lookup_candidates(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |p: String| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    push(path.to_string());
    if let Ok(c) = dunce::canonicalize(&Path::new(path)) {
        push(c.to_string_lossy().into_owned());
    }
    if let Some(rest) = path.strip_prefix("/private") {
        if rest.starts_with('/') {
            push(rest.to_string());
        }
    } else if path.starts_with('/') {
        push(format!("/private{path}"));
    }
    out
}

/// Match a path to a registered `projects.path`, including macOS `/var` ↔
/// `/private/var` aliasing.
pub fn lookup_project_row(path: &str) -> Option<(String, ProjectRow)> {
    path_lookup_candidates(path)
        .into_iter()
        .find_map(|c| lookup_exact(&c).map(|row| (c, row)))
}

/// The refusal when an agent's hire names a path that is already there
/// (403 `owner_only`: a person changes an existing workspace).
fn exists_refusal(path: &Path, what: &str) -> HireError {
    HireError::new(
        403,
        "owner_only",
        format!(
            "{} {what}. An agent's hire only makes a new workspace: pick a folder that doesn't exist yet (or another --name). A person changes an existing workspace: Settings → Workspaces / Agents → (workspace), or `{cli} agent hire <dir>` from a terminal outside a {product} agent session.",
            path.display(),
            cli = crate::brand::CLI_NAME,
            product = crate::brand::PRODUCT_NAME,
        ),
    )
}

/// The folders the owner set up, not the agent: the home folder and the
/// hire root. They and their ancestors may be symlinks (`/home` on some
/// systems); a folder below them may not.
fn owner_bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(h) = crate::paths::home_dir() {
        out.push(h);
    }
    if let Ok(r) = super::root::hire_root() {
        out.push(r);
    }
    out
}

/// Refuse a symlink anywhere in `path`, looked at without following it,
/// except the owner's folders and their ancestors ([`owner_bases`]). Stops at
/// the first part that doesn't exist yet.
fn refuse_symlinks(path: &Path) -> Result<(), HireError> {
    let bases = owner_bases();
    let mut cur = PathBuf::new();
    for part in path.components() {
        cur.push(part);
        if bases.iter().any(|b| b.starts_with(&cur)) {
            continue;
        }
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(HireError::new(
                    403,
                    "owner_only",
                    format!(
                        "{} is a symlink. An agent's hire names a real folder path with no symlinks in it, and only makes a new workspace.",
                        cur.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(HireError::usage(format!("{}: {e}", cur.display()))),
        }
    }
    Ok(())
}

/// An agent's hire only ever creates. Check the path before anything is
/// written: `~` expanded, absolute, no `..`, normalized (`.` parts, doubled
/// and trailing slashes dropped), no symlink in it, nothing there yet (a
/// folder, a file or a link), and not a registered workspace in this
/// spelling or its real one. [`claim_new_folder`] makes the folder in the
/// same step as the last check.
pub fn create_only_path(raw: &str) -> Result<String, HireError> {
    let expanded = expand_tilde(raw)?;
    if expanded.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(HireError::usage("an agent's hire path may not contain '..'"));
    }
    let path: PathBuf = expanded.components().collect();
    if path.parent().is_none() {
        return Err(HireError::usage("an agent's hire path can't be /"));
    }
    refuse_symlinks(&path)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err(exists_refusal(&path, "already exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(HireError::usage(format!("{}: {e}", path.display()))),
    }
    let spelled = path.to_string_lossy().into_owned();
    let real = super::root::resolve_existing_prefix(&path).to_string_lossy().into_owned();
    if lookup_project_row(&spelled).is_some() || lookup_project_row(&real).is_some() {
        return Err(exists_refusal(&path, "is already a registered workspace"));
    }
    Ok(spelled)
}

/// Make the new folder for an agent's hire. Its parents may be made; the
/// folder itself is made with `create_dir`, so a folder or link that
/// appeared after [`create_only_path`] is refused (`AlreadyExists`), never
/// adopted.
pub fn claim_new_folder(path: &str) -> Result<(), HireError> {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| HireError::usage(format!("create parent: {e}")))?;
    }
    // Again, now the parents exist: none of them is a symlink.
    refuse_symlinks(p)?;
    match fs::create_dir(p) {
        Ok(()) => super::folder_walk::set_new_folder_mode(p).map_err(|e| HireError::usage(format!("create {}: {e}", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(exists_refusal(p, "already exists")),
        Err(e) => Err(HireError::usage(format!("create {}: {e}", p.display()))),
    }
}

/// A new workspace must sit inside `auth.new_workspace_root` when the
/// caller has one (an API key: the hire root or its sub-folder; an agent
/// passport: the hire root). `outside_hire_root`, before any write.
pub fn check_new_workspace_root(path: &str, auth: &ConvergeAuthority) -> Result<(), HireError> {
    let Some(root) = auth.new_workspace_root.as_ref() else {
        return Ok(());
    };
    if super::root::is_under(Path::new(path), root) {
        return Ok(());
    }
    let hint = if auth.create_only {
        format!(
            "{path} is outside the hire root. An agent hires only inside the hire root {}: pick a new folder there (or leave the folder out with a template). A person hires anywhere, or sets the root: `{} agent hire-root set <dir>`.",
            root.display(),
            crate::brand::CLI_NAME
        )
    } else {
        format!(
            "a new workspace made with an API key must be inside the hire root {} (the owner sets it: k2 agent hire-root set <dir>)",
            root.display()
        )
    };
    Err(HireError::new(403, "outside_hire_root", hint))
}

/// A key's refusal for a link (or a file) below its root.
fn key_link_refusal(at: &Path, root: &Path) -> HireError {
    HireError::new(
        403,
        "outside_hire_root",
        format!(
            "{} is a link or not a folder: an API key's hire names a real folder inside the hire root {}, with no links below the root",
            at.display(),
            root.display()
        ),
    )
}

/// Split a key's folder at its root: the root's real path and the plain
/// names below it. The first part of `path` that is the root (in any
/// spelling) ends the owner's part; everything after it is checked.
fn below_root(path: &Path, root: &Path) -> Result<(PathBuf, Vec<std::ffi::OsString>), HireError> {
    let outside = || {
        HireError::new(
            403,
            "outside_hire_root",
            format!("{} is not a folder inside the hire root {}", path.display(), root.display()),
        )
    };
    let root_real = super::root::resolve_existing_prefix(root);
    let parts: Vec<Component> = path.components().collect();
    let mut cur = PathBuf::new();
    for (i, part) in parts.iter().enumerate() {
        cur.push(part);
        if super::root::resolve_existing_prefix(&cur) != root_real {
            continue;
        }
        let mut rel = Vec::new();
        for p in &parts[i + 1..] {
            match p {
                Component::Normal(n) => rel.push(n.to_os_string()),
                Component::CurDir => {}
                _ => return Err(outside()),
            }
        }
        if rel.is_empty() {
            break;
        }
        return Ok((root_real, rel));
    }
    Err(outside())
}

/// No link anywhere below the root, looked at without following it. Stops
/// at the first part that isn't there yet.
fn refuse_links_below(root_real: &Path, rel: &[std::ffi::OsString]) -> Result<(), HireError> {
    let mut at = root_real.to_path_buf();
    for name in rel {
        at.push(name);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return Err(key_link_refusal(&at, root_real)),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(HireError::usage(format!("{}: {e}", at.display()))),
        }
    }
    Ok(())
}

fn walk_refusal(e: super::folder_walk::WalkError, root_real: &Path) -> HireError {
    use super::folder_walk::WalkError;
    match e {
        WalkError::NotAFolder(at) | WalkError::Missing(at) => key_link_refusal(&at, root_real),
        WalkError::Unsafe(at) => super::root::unsafe_folder_error(&at),
        WalkError::AlreadyThere(at) => HireError::new(
            403,
            "existing_workspace",
            format!("{} appeared while this hire was making it: pick another folder", at.display()),
        ),
        WalkError::Io(at, e) => HireError::usage(format!("{}: {e}", at.display())),
    }
}

/// An API key's hire of a folder that isn't registered: made the way an
/// agent's is (no links below the key's root, the folder made in one step)
/// or, when it is there already, taken on through a walk from the root
/// that refuses links; never a folder another user could change, and for
/// a template hire only an empty one. Registered only while the path still
/// walks to that same folder. Returns the registered path.
fn key_folder(
    path: &str,
    root: &Path,
    new_only: bool,
    register: &dyn Fn(&str) -> Result<String, String>,
    steps_out: &mut Vec<StepReport>,
) -> Result<String, HireError> {
    super::root::ensure_root(root)?;
    let (root_real, rel) = below_root(Path::new(path), root)?;
    refuse_links_below(&root_real, &rel)?;
    let target = rel.iter().fold(root_real.clone(), |p, n| p.join(n));
    let target_s = target.to_string_lossy().into_owned();
    let make = match fs::symlink_metadata(&target) {
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(HireError::usage(format!("{}: {e}", target.display()))),
    };
    // Only a folder this hire made goes again, and only while it is empty
    // (`remove_dir` never follows a link).
    let undo = |made: bool| {
        if made {
            let _ = fs::remove_dir(&target);
        }
    };
    let first = super::folder_walk::walk(&root_real, &rel, make).map_err(|e| walk_refusal(e, &root_real))?;
    if new_only && !first.made && !first.empty {
        return Err(HireError::new(
            403,
            "existing_workspace",
            format!("{target_s} already exists: an API key hires only into a new, empty folder (leave out <dir> for <hire root>/<handle>)"),
        ));
    }
    steps_out.push(if first.made { done("create-dir") } else { skipped("create-dir", "exists") });
    #[cfg(test)]
    test_hook::run(test_hook::Stage::BeforeRegister, &target_s);
    let same = || super::folder_walk::walk(&root_real, &rel, false).map(|r| r.id == first.id);
    match same() {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            undo(first.made);
            return Err(key_link_refusal(&target, &root_real));
        }
    }
    if let Err(e) = register(&target_s) {
        undo(first.made);
        if crate::workspace::lifecycle::is_already_registered(&e) {
            return Err(HireError::new(403, "existing_workspace", format!("{target_s} is already a workspace: an API key's hire makes or takes on a folder that isn't one")));
        }
        return Err(HireError::usage(e));
    }
    // Registered: still the folder the walk opened, or the registration goes.
    if !matches!(same(), Ok(true)) {
        let _ = crate::workspace::lifecycle::remove_workspace_db_only(&target_s);
        return Err(key_link_refusal(&target, &root_real));
    }
    steps_out.push(done("register-workspace"));
    Ok(target_s)
}

/// The authority checks that need only the request (before any write).
pub fn check_db_access(
    caller: &HireCaller,
    requested: Option<&str>,
    max_db: Option<&'static str>,
) -> Result<(), HireError> {
    let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    if !super::recipe::DB_ACCESS.contains(&req) {
        return Err(HireError::usage("dbAccess must be 'off', 'read', or 'write'"));
    }
    let rank = super::recipe::db_rank(req);
    if caller.is_agent() && rank > 0 {
        // 0.44.4, now in the daemon: an agent never grants itself (or an
        // agent it hires) database access. Lowering to `off` is fine.
        return Err(HireError::new(
            403,
            "owner_only",
            "An agent can't grant 'db_agent_access'. Ask a human (owner or admin): Settings → Workspaces / Agents → (workspace) → Permissions → Database → \"Let this agent create databases\", or `k2 agent hire <dir> --db-access write` from a terminal outside a K2 agent session.",
        ));
    }
    if let Some(max) = max_db {
        if rank > super::recipe::db_rank(max) {
            return Err(HireError::new(
                403,
                "exceeds_key",
                format!("dbAccess '{req}' is above this key's DB door ('{max}'); the owner mints a key with the db door to allow it"),
            ));
        }
    }
    Ok(())
}

/// "Manage Sew" at hire time (S30 K19): the owner, an Owner/Admin login or
/// a hire key may set it; an agent caller never turns it on (it would hand
/// an agent it hires, or itself, the Sew admin plane). Off is always fine.
pub fn check_chat_manage(caller: &HireCaller, requested: Option<bool>) -> Result<(), HireError> {
    if requested == Some(true) && caller.is_agent() {
        return Err(HireError::new(
            403,
            "owner_only",
            "An agent can't turn on 'Manage chat and voice' (chat_manage). Ask a human (owner or admin) to run `k2 agent access set <workspace> chat_manage on` from a terminal outside a K2 agent session (it has no Settings row).",
        ));
    }
    Ok(())
}

/// Create-or-converge one workspace.
pub fn converge(
    req: &ConvergeRequest,
    caller: &HireCaller,
    auth: &ConvergeAuthority,
) -> Result<ConvergeOutcome, HireError> {
    let (mut path, existing) = if auth.create_only {
        (create_only_path(&req.path)?, None)
    } else {
        let expanded = expand_hire_path(&req.path)?;
        match lookup_project_row(&expanded) {
            Some((registered, row)) => (registered, Some(row)),
            None => (expanded, None),
        }
    };
    // An API key's hire of a folder that isn't a workspace yet: its own
    // branch below (a plain converge of a granted workspace stays as it was).
    let key_root = match (caller.api_key_id(), auth.new_workspace_root.as_deref()) {
        (Some(_), Some(root)) if existing.is_none() && !auth.create_only => Some(root.to_path_buf()),
        _ => None,
    };
    // First, for a new workspace inside a hire root (a key's, an agent's)
    // by a caller that may make one: no other user can change a folder on
    // the way to the root, before any folder there is looked at or taken
    // on. A caller that may not create gets that refusal below.
    if existing.is_none() && auth.may_create {
        if let Some(root) = auth.new_workspace_root.as_deref() {
            super::root::check_root_safe(root)?;
        }
    }
    // Then: a key's template hire makes a new workspace only (an agent's
    // hire is create_only, above).
    if auth.new_only {
        check_new_folder(caller, &path)?;
    }
    let template = req.template.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let persona = req.persona.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let persona_raw = req.persona_raw.as_deref().filter(|s| !s.trim().is_empty());
    if (template.is_some() as u8 + persona.is_some() as u8 + persona_raw.is_some() as u8) > 1 {
        return Err(HireError::usage("template and persona are mutually exclusive — pass one or the other"));
    }
    let tpl = match template {
        Some(t) => {
            let tpl = super::template::resolve(t)?;
            super::template::check_caller(&tpl, caller)?;
            let vars = super::template::bind_params(&tpl, &req.params.clone().unwrap_or_default(), caller)?;
            let answers = super::template::bind_answers(&tpl, &req.answers.clone().unwrap_or_default())?;
            let declined = super::template::permissions::clean_declined(&req.decline_permissions.clone().unwrap_or_default());
            super::template::permissions::check_declined(&tpl, &declined)?;
            let allowed = super::template::permissions::clean_declined(&req.allow_permissions.clone().unwrap_or_default());
            super::template::permissions::check_allowed(&tpl, &allowed, &declined)?;
            super::template::apply::check_lock(&path, &tpl)?;
            Some((tpl, vars, answers, declined, allowed))
        }
        None => {
            if req.decline_permissions.as_ref().is_some_and(|d| !d.is_empty()) {
                return Err(HireError::usage("declinePermissions goes with a template (--template <id>)"));
            }
            if req.allow_permissions.as_ref().is_some_and(|d| !d.is_empty()) {
                return Err(HireError::usage("allowPermissions goes with a template (--template <id>)"));
            }
            if req.params.as_ref().is_some_and(|p| !p.is_empty()) {
                return Err(HireError::usage("params go with a template (--template <id>)"));
            }
            if req.answers.as_ref().is_some_and(|a| !a.is_empty()) {
                return Err(HireError::usage("answers go with a template (--template <id>)"));
            }
            None
        }
    };
    check_db_access(caller, req.db_access.as_deref(), auth.max_db)?;
    let preset = steps::check_preset(req.preset.as_deref())?;
    if existing.is_none() {
        if !auth.may_create {
            return Err(HireError::new(
                403,
                "usage",
                auth.create_refusal.clone().unwrap_or_else(|| "this caller can't create a new workspace".into()),
            ));
        }
        check_new_workspace_root(&path, auth)?;
    }

    let no_wiki = req.no_wiki.unwrap_or(false);
    let seed_agents_md = !req.no_agents_md.unwrap_or(false);
    let fanout = req.fanout.unwrap_or(false);
    let mut changed = false;
    let mut created = false;
    let mut steps_out: Vec<StepReport> = Vec::new();

    if existing.is_none() && auth.create_only {
        if let Some(root) = auth.new_workspace_root.as_deref() {
            super::root::ensure_root(root)?;
        }
        claim_new_folder(&path)?;
        steps_out.push(done("create-dir"));
        match crate::workspace::lifecycle::open_workspace_ex(&path, !no_wiki, seed_agents_md, fanout) {
            Ok(_) => {
                changed = true;
                created = true;
                steps_out.push(done("register-workspace"));
            }
            Err(e) => {
                // Registered by someone else since the check: never adopted.
                // The folder this call made goes again only if still empty.
                let _ = fs::remove_dir(&path);
                if crate::workspace::lifecycle::is_already_registered(&e) {
                    return Err(exists_refusal(Path::new(&path), "is already a registered workspace"));
                }
                return Err(HireError::usage(e));
            }
        }
    } else if let Some(root) = key_root.as_deref() {
        #[cfg(test)]
        test_hook::run(test_hook::Stage::AfterChecks, &path);
        let register = |p: &str| crate::workspace::lifecycle::open_workspace_ex(p, !no_wiki, seed_agents_md, fanout);
        path = key_folder(&path, root, auth.new_only, &register, &mut steps_out)?;
        changed = true;
        created = true;
    } else if existing.is_none() {
        // A new workspace inside the hire root: a root that isn't there yet
        // is made the way the daemon makes one (the server user's, 0700).
        if let Ok(root) = super::root::hire_root() {
            if super::root::is_under(Path::new(&path), &root) {
                super::root::ensure_root(&root)?;
            }
        }
        let dir_exists = Path::new(&path).exists();
        let result = if dir_exists {
            steps_out.push(skipped("create-dir", "exists"));
            crate::workspace::lifecycle::open_workspace_ex(&path, !no_wiki, seed_agents_md, fanout)
        } else {
            if let Some(parent) = Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| HireError::usage(format!("create parent: {e}")))?;
                }
            }
            steps_out.push(done("create-dir"));
            crate::workspace::lifecycle::create_workspace_ex(&path, !no_wiki, seed_agents_md, fanout)
        };
        match result {
            Ok(_) => {
                changed = true;
                created = true;
                steps_out.push(done("register-workspace"));
            }
            Err(e) if crate::workspace::lifecycle::is_already_registered(&e) => {
                steps_out.push(skipped("register-workspace", "already-registered"));
            }
            Err(e) => return Err(HireError::usage(e)),
        }
    } else {
        steps_out.push(skipped("create-dir", "exists"));
        steps_out.push(skipped("register-workspace", "already-registered"));
    }

    if !no_wiki {
        let seeded = crate::wiki::seed_wiki(Path::new(&path)).map_err(|e| HireError::usage(format!("seed wiki: {e}")))?;
        if !seeded.is_empty() {
            changed = true;
        }
    }

    let mut template_out = None;
    if let Some((tpl, vars, answers, declined, allowed)) = tpl.as_ref() {
        // The template writes the persona (and its starters, layers, lock)
        // before the name and preset steps below.
        let display = match req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(n) => n.to_string(),
            None => {
                let d = crate::workspace::display::agent_display_name(&path);
                if d.is_empty() {
                    Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                } else {
                    d
                }
            }
        };
        let out = super::template::apply::workspace_steps(&super::template::apply::StepInput {
            tpl,
            vars,
            answers,
            path: &path,
            caller,
            display_name: display,
            created,
            registered: true,
            dry_run: false,
            no_wiki,
            tool_chosen: req.preset.as_deref().is_some_and(|s| !s.trim().is_empty()),
            model_chosen: req.default_model.as_deref().is_some_and(|s| !s.trim().is_empty()),
            declined,
            allowed,
            permissions_by_door: req.permissions_by_door,
        })?;
        changed |= out.changed;
        steps_out.push(if out.wrote_role { done("write-persona") } else { skipped("write-persona", "exists") });
        template_out = Some(out);
    } else {
        let source = if let Some(p) = persona {
            PersonaSource::Body(p.to_string())
        } else if let Some(raw) = persona_raw {
            PersonaSource::Raw(raw.to_string())
        } else {
            PersonaSource::Default
        };
        if steps::ensure_persona(&path, req.role.as_deref(), &source)? {
            changed = true;
            steps_out.push(done("write-persona"));
        } else {
            steps_out.push(skipped("write-persona", "exists"));
        }
    }

    if let Some(name) = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if steps::set_display_name(&path, name)? {
            changed = true;
            steps_out.push(done("set-name"));
        } else {
            steps_out.push(skipped("set-name", "already named"));
        }
    }
    if let Some(preset) = preset.as_deref() {
        let did = steps::set_setting(&path, "default_agent", preset)?;
        changed |= did;
        steps_out.push(if did { done("set-default-agent") } else { skipped("set-default-agent", "already set") });
    }
    if let Some(model) = req.default_model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let did = steps::set_setting(&path, "default_model", model)?;
        changed |= did;
        steps_out.push(if did { done("set-model") } else { skipped("set-model", "already set") });
    }

    let mut wiki_notes = Vec::new();
    if let Some(notes) = req.wiki.as_ref() {
        wiki_notes = steps::write_wiki_notes(&path, notes, caller)?;
        if !wiki_notes.is_empty() {
            changed = true;
        }
    }
    if let Some(ids) = req.context.as_ref() {
        changed |= steps::add_context(&path, ids)?;
    }
    if let Some(layers) = req.layers.as_ref() {
        changed |= steps::add_inline_layers(&path, layers)?;
    }
    if let Some(access) = req.db_access.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let did = steps::set_setting(&path, "db_agent_access", access)?;
        changed |= did;
        steps_out.push(if did { done("set-db-access") } else { skipped("set-db-access", "already set") });
    }

    let row = lookup_project_row(&path)
        .map(|(_, r)| r)
        .ok_or_else(|| HireError::usage("workspace did not register"))?;
    let basename = Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let handle = row
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&basename)
        .to_string();
    let display = {
        let d = crate::workspace::display::agent_display_name(&path);
        if d.is_empty() {
            row.name.clone()
        } else {
            d
        }
    };
    Ok(ConvergeOutcome {
        path,
        id: row.id,
        handle,
        name: display,
        changed,
        created,
        steps: steps_out,
        wiki_notes,
        template: template_out,
    })
}

/// Tests: a step run at a named point of a hire that makes or opens a
/// folder, so a test can change the filesystem there.
#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Stage {
        /// After the root and new-folder checks, before any folder is
        /// made or opened.
        AfterChecks,
        /// Right before the folder is registered.
        BeforeRegister,
    }

    type Hook = Box<dyn FnMut(Stage, &str)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Set (or clear, with `None`) this thread's hook.
    pub(crate) fn set(hook: Option<Hook>) {
        HOOK.with(|h| *h.borrow_mut() = hook);
    }

    pub(crate) fn run(stage: Stage, path: &str) {
        let taken = HOOK.with(|h| h.borrow_mut().take());
        if let Some(mut f) = taken {
            f(stage, path);
            HOOK.with(|h| {
                let mut slot = h.borrow_mut();
                if slot.is_none() {
                    *slot = Some(f);
                }
            });
        }
    }
}

#[cfg(test)]
#[path = "converge_tests.rs"]
mod key_hire_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_callers_never_raise_db_access() {
        let agent = HireCaller::Agent { workspace_id: "w".into() };
        assert_eq!(check_db_access(&agent, Some("write"), None).unwrap_err().code, "owner_only");
        assert_eq!(check_db_access(&agent, Some("read"), None).unwrap_err().code, "owner_only");
        assert!(check_db_access(&agent, Some("off"), None).is_ok());
        assert!(check_db_access(&HireCaller::Owner, Some("write"), None).is_ok());
        assert_eq!(check_chat_manage(&agent, Some(true)).unwrap_err().code, "owner_only");
        assert!(check_chat_manage(&agent, Some(false)).is_ok() && check_chat_manage(&agent, None).is_ok());
        assert!(check_chat_manage(&HireCaller::Owner, Some(true)).is_ok());
        assert!(check_chat_manage(&HireCaller::ApiKey { id: "k".into(), label: None }, Some(true)).is_ok());
    }

    #[test]
    fn key_db_door_caps_db_access() {
        let key = HireCaller::ApiKey { id: "k".into(), label: None };
        assert_eq!(check_db_access(&key, Some("read"), Some("off")).unwrap_err().code, "exceeds_key");
        assert_eq!(check_db_access(&key, Some("write"), Some("read")).unwrap_err().code, "exceeds_key");
        assert!(check_db_access(&key, Some("read"), Some("read")).is_ok());
        assert!(check_db_access(&key, Some("off"), Some("off")).is_ok());
        assert_eq!(check_db_access(&key, Some("admin"), None).unwrap_err().code, "usage");
    }
}
