//! Which file a docs call names, and whether it may be a live doc
//! (Markdown multiplayer v1, K2; vs-live MI8, MI12).
//!
//! Every caller is normalised to `(project id, workspace-relative path)`,
//! the kit's [`DocKey`]:
//! - a relative `path` resolves under `workspace` (a handle, name, id or
//!   root path; an agent's own workspace when it names none);
//! - an absolute `path` resolves to the registered workspace whose root
//!   holds it (the longest root wins).
//!
//! Then the jail: the file must exist, and after symlinks it must still be
//! inside the workspace root (`fs_routes::jail_rel_path`). The kit re-checks
//! nothing about paths (it never touches disk in this slice).
//!
//! **Which tier, by file name** (the text pack's HOST-INTEGRATION §1): the
//! more specific pack first, `.md` / `.markdown` → the Markdown pack;
//! then any text, by the text pack's table (`TextPack::for_path`: code,
//! config, markup, diagram sources, `.txt`, unknown names, and `.csv` /
//! `.tsv` as `csv-as-text` until a grid pack exists) → the text pack (tier
//! 1, live editing); everything else (images, media, office files,
//! archives, binaries) → a tier-0 doc (the kit's blob docs:
//! presence, a lease, versions and restore), up to
//! [`super::blob_max_bytes`] (`too_large` past it). `.mdx` is JSX, so it is
//! tier 0. A text file a pack refuses at open (`too_large`, `not_utf8`,
//! `binary`, `mixed_line_endings`) falls back to tier 0 with the reason
//! shown (`routes`), never to a plain editor. A path keeps its kind for
//! life (`mv` can't change Markdown, text or any-file). A `.pptx` deck is a
//! live keyed doc ([`Tier::Deck`], the pptx pack) while the Slides core is
//! on, and a tier-0 doc while it is off ([`tier_now`]); so are an `.otio`
//! timeline ([`Tier::Timeline`], the timeline core) and a `.blend` scene
//! ([`Tier::Scene`], the blend core). The other files those cores and the
//! fab core read (`.fcpxml`, `.prproj`, photoplot and drill files) are
//! tier-0 docs by name, never live text, indexed by their pack while their
//! core is on (`blob_packs`).
//!
//! **Never live docs** (MI12), refused before any open, on every tier:
//! - secret files by path or content (`secret_paths::classify`), `secret`;
//! - `.k2/vault/**`, `protected`;
//! - files K2 generates and rewrites (AGENTS.md, its own skills, rosters,
//!   the harness mirrors), `generated`. An edit there would be overwritten
//!   by the next recompose.

use std::path::{Path, PathBuf};

use k2_mpc_base::store::DocKey;

/// A `.pptx` deck, by its name alone (any case, a name before the dot).
pub fn is_deck_file(rel: &str) -> bool {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("pptx"))
}

/// An `.otio` timeline, by its name alone (any case, a name before the dot).
pub fn is_timeline_file(rel: &str) -> bool {
    super::blob_packs::kind_by_name(rel) == super::blob_packs::BlobKind::Otio
}

/// A `.blend` scene, by its name alone (any case, a name before the dot).
pub fn is_scene_file(rel: &str) -> bool {
    super::blob_packs::kind_by_name(rel) == super::blob_packs::BlobKind::Blend
}

/// A path's tier now: [`Tier::of`], except that a live keyed doc is an
/// any-file doc while its core is off (a deck with the Slides core, a
/// timeline with the timeline core, a scene with the blend core: as before
/// those cores), so every route, export and the Live socket treat it
/// exactly as they did.
pub fn tier_now(rel: &str) -> Tier {
    match Tier::of(rel) {
        t @ (Tier::Deck | Tier::Timeline | Tier::Scene) if !super::cores::is_on(super::cores::core_of(t)) => Tier::Blob,
        t => t,
    }
}

/// A Board file, by its name alone (any case, whatever comes before it).
pub fn is_board_file(rel: &str) -> bool {
    const EXT: &[u8] = b".k5board";
    let b = rel.as_bytes();
    b.len() >= EXT.len() && b[b.len() - EXT.len()..].eq_ignore_ascii_case(EXT)
}

/// Design, CAD and 3D files, by extension: any-file docs (K2's list, beside
/// the text pack's `NOT_TEXT`, which has only `psd`, `blend`, `glb` and
/// `3mf`). The pack calls an extension it doesn't know text if the bytes pass
/// its guard, so without this a binary `.dwg` went to the Live editor to be
/// refused, and an ASCII `.step` or `.obj` got line merges that break the
/// model. None of these is in the pack's table (a test checks).
const DESIGN_EXTENSIONS: &[&str] = &[
    // layered images and vector art
    "psb", "ai", "indd", "xcf", "kra", "sketch", "afdesign", "afphoto",
    // CAD (`.dxf`, `.step`/`.stp`, `.iges`/`.igs` and `.ifc` are on the text
    // pack's own not-text list since text v0.2.22, so they aren't here)
    "dwg", "dgn", "sat", "sldprt", "sldasm", "slddrw", "ipt", "iam", "idw", "f3d", "f3z",
    "3dm", "skp", "fcstd", "catpart", "catproduct", "x_t", "x_b", "rvt", "rfa",
    // 3D models and scenes (`.obj` is the text pack's not-text list's)
    "stl", "ply", "gltf", "fbx", "dae", "usd", "usda", "usdc", "usdz", "abc", "3ds", "c4d", "max", "ma", "mb", "lwo",
    "blend1",
];

/// A design, CAD or 3D file, by its name alone (any case).
pub fn is_design_file(rel: &str) -> bool {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    name.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && DESIGN_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// How much of the kit a file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Markdown (tier 1): live co-editing, text tools, undo.
    Markdown,
    /// Any other text (tier 1, the text pack): live co-editing, text tools,
    /// undo; `family` picks the editor mode.
    Text(k2_mpc_text::Family),
    /// Any other file (tier 0): presence, a lease, versions and restore.
    Blob,
    /// A `.pptx` deck as a live keyed doc (K2 Cores' pptx pack, `PptxLive`),
    /// behind the Slides core: slides and shapes by their own ids, the pack's
    /// verbs for agents, no live view for people yet. While that core is off
    /// a deck is [`Tier::Blob`] ([`tier_now`]).
    Deck,
    /// An `.otio` timeline as a live keyed doc (the timeline pack,
    /// `TimelineLive`), behind the timeline core: tracks, items, markers and
    /// media by their own keys, the pack's clip and marker verbs for agents.
    /// People watch it (and follow a playhead). While that core is off it is
    /// [`Tier::Blob`].
    Timeline,
    /// A `.blend` scene as a live keyed doc (the BLEND pack, `BlendLive`),
    /// behind the blend core: collections and objects by their own keys, the
    /// pack's object verbs for agents, 3D previews from its mesh read. People
    /// watch it. While that core is off it is [`Tier::Blob`].
    Scene,
}

impl Tier {
    /// The tier a workspace-relative path gets (by name only).
    pub fn of(rel: &str) -> Tier {
        // First: `.env.k5board` and `Makefile.k5board` match the text pack's
        // name prefixes, and a board is a zip, never text.
        if is_board_file(rel) || is_design_file(rel) {
            Tier::Blob
        } else if is_deck_file(rel) {
            Tier::Deck
        } else if is_timeline_file(rel) {
            Tier::Timeline
        } else if is_scene_file(rel) {
            Tier::Scene
        } else if super::blob_packs::is_pack_file(rel) {
            // `.fcpxml`, `.prproj`, photoplot and drill files: structured
            // files whose lines point at each other, never live text.
            Tier::Blob
        } else if k2_mpc_markdown::is_markdown_path(rel) {
            Tier::Markdown
        } else if let Some(pack) = k2_mpc_text::TextPack::for_path(rel) {
            Tier::Text(pack.family())
        } else {
            Tier::Blob
        }
    }

    /// On the wire: `1` (a live doc: Markdown, the text pack, a deck, a
    /// timeline, a scene) or `0` (any file).
    pub fn number(self) -> u8 {
        match self {
            Tier::Markdown | Tier::Text(_) | Tier::Deck | Tier::Timeline | Tier::Scene => 1,
            Tier::Blob => 0,
        }
    }

    /// A live keyed doc that people watch and agents change with the pack's
    /// verbs: a deck, a timeline or a scene.
    pub fn is_keyed(self) -> bool {
        matches!(self, Tier::Deck | Tier::Timeline | Tier::Scene)
    }

    /// The pack's name on the wire: `markdown`, `text`, `pptx`,
    /// `timeline-otio`, `blend` or `blob`. For a keyed doc it is the pack's
    /// schema name, which the kit stamps on its stored doc.
    pub fn pack(self) -> &'static str {
        match self {
            Tier::Markdown => "markdown",
            Tier::Text(_) => "text",
            Tier::Deck => "pptx",
            Tier::Timeline => k2_mpc_timeline::live::SCHEMA.name,
            Tier::Scene => k2_mpc_blend::live::SCHEMA.name,
            Tier::Blob => "blob",
        }
    }

    /// The editor family (`markdown`, the text pack's `text` / `code` /
    /// `config` / `markup` / `diagram` / `csv-as-text`, `deck`, `timeline`,
    /// `scene`, or `none`).
    pub fn family(self) -> &'static str {
        match self {
            Tier::Markdown => "markdown",
            Tier::Text(f) => f.name(),
            Tier::Deck => "deck",
            Tier::Timeline => "timeline",
            Tier::Scene => "scene",
            Tier::Blob => "none",
        }
    }

    /// The same kind of doc (`mv` keeps it): Markdown, text (any family:
    /// one schema), a deck, a timeline, a scene, or any-file.
    pub fn same_kind(self, other: Tier) -> bool {
        matches!(
            (self, other),
            (Tier::Markdown, Tier::Markdown)
                | (Tier::Text(_), Tier::Text(_))
                | (Tier::Deck, Tier::Deck)
                | (Tier::Timeline, Tier::Timeline)
                | (Tier::Scene, Tier::Scene)
                | (Tier::Blob, Tier::Blob)
        )
    }
}

/// A resolved, jailed doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub project_id: String,
    /// Canonical workspace root.
    pub root: PathBuf,
    /// Workspace-relative path, `/`-separated (the doc key's path).
    pub rel: String,
    /// Canonical absolute file path.
    pub abs: PathBuf,
    pub tier: Tier,
}

impl Target {
    pub fn key(&self) -> DocKey {
        DocKey { space: self.project_id.clone(), path: self.rel.clone() }
    }
}

/// Why a path can't be a doc. `code` goes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Refusal { status: "400 Bad Request", code, message: message.into() }
    }
    fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Refusal { status: "404 Not Found", code, message: message.into() }
    }
    fn declined(code: &'static str, message: impl Into<String>) -> Self {
        Refusal { status: "422 Unprocessable Entity", code, message: message.into() }
    }
    /// A Board file isn't a live doc: a text or tier-0 write would put raw
    /// bytes on disk past the format's codec, so `k2 doc` never opens one.
    /// [`share_rel`] still takes it (a share names a file, it opens nothing).
    fn board_file(rel: &str) -> Self {
        Refusal::declined("board_file", format!("{rel} is a Board file; k2 doc doesn't read or change Board files"))
    }
}

/// Files K2 writes itself (vs-live §3.2). Workspace-relative, lowercase
/// compared. `*` matches one path segment.
const GENERATED: &[&str] = &[
    ".k2/agents.md",
    ".k2/skills/k2-cli/skill.md",
    ".k2/skills/k2-canonical-agents/skill.md",
    ".k2/skills/k2-zen/skill.md",
    ".k2/skills/k2-permissions/skill.md",
    ".k2/skills/k2so/skill.md",
    ".k2/context/catalog/*-roster.md",
    ".k2/context/catalog/apps-maintained.md",
    ".claude/skills/*/skill.md",
    ".opencode/agent/*.md",
    ".pi/skills/*/skill.md",
];

/// Root-level harness mirrors K2 keeps as symlinks to `.k2/` files
/// (`workspace/harness.rs`). Generated only when they are symlinks; a
/// person's own `CLAUDE.md` file is theirs.
const MIRRORS: &[&str] = &["claude.md", "gemini.md", "agent.md", "agents.md"];

fn segment_match(pattern: &str, seg: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == seg,
        Some((pre, post)) => seg.len() >= pre.len() + post.len() && seg.starts_with(pre) && seg.ends_with(post),
    }
}

/// `rel` lower-cased, with a first segment that names the workspace's
/// private folder under any of its names (`.k2so`, `.k5`, `.K2`, `.k2.`:
/// `k2_core::paths::is_workspace_dot_dir`) written as `.k2`. The lists
/// below say `.k2/…`; a leftover `.k2so/` beside it holds the same kinds
/// of files, and root links may still point into it, so the same rules
/// apply there.
fn with_k2_dot_dir(rel: &str) -> String {
    let lower = rel.to_ascii_lowercase();
    let (first, rest) = match lower.split_once('/') {
        Some((first, rest)) => (first, Some(rest)),
        None => (lower.as_str(), None),
    };
    if !k2_core::paths::is_workspace_dot_dir(first) {
        return lower;
    }
    match rest {
        Some(rest) => format!(".k2/{rest}"),
        None => ".k2".to_string(),
    }
}

/// True for a path K2 generates (lowercase compare; `*` = one segment).
pub fn is_generated(rel: &str) -> bool {
    let lower = with_k2_dot_dir(rel);
    let segs: Vec<&str> = lower.split('/').collect();
    GENERATED.iter().any(|g| {
        let gs: Vec<&str> = g.split('/').collect();
        gs.len() == segs.len() && gs.iter().zip(&segs).all(|(p, s)| segment_match(p, s))
    })
}

/// True under `.k2/vault/` (or the same folder under `.k2so/`, `.k5/`).
pub fn is_vault(rel: &str) -> bool {
    let lower = with_k2_dot_dir(rel);
    lower == ".k2/vault" || lower.starts_with(".k2/vault/")
}

/// True inside any `.git` folder: git's database, its config and its hooks
/// (a written hook would run as the workspace's user).
pub fn is_git_internal(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let segs: Vec<&str> = lower.split('/').collect();
    segs.len() > 1 && segs[..segs.len() - 1].contains(&".git")
}

/// `.k2/` folders whose other files may be tier-0 docs (wiki attachments,
/// PRD images), and the conflict copies' folder (`.k2/conflicts/`,
/// [`super::host::conflict_dir_for`]: someone's version of a doc, kept when
/// an outside save replaced it, opened as a doc of its own: kit v0.19.0's
/// `outside_save.copy.doc`). Everything else under `.k2/` is K2's
/// configuration (settings, heartbeats, hooks, rosters): never written
/// through a doc.
const K2_DOC_DIRS: &[&str] = &[".k2/wiki/", ".k2/prds/", ".k2/docs/", ".k2/conflicts/"];

/// True for a path under `.k2/` (or `.k2so/`, `.k5/`) outside
/// [`K2_DOC_DIRS`].
pub fn is_k2_config(rel: &str) -> bool {
    let lower = with_k2_dot_dir(rel);
    lower.starts_with(".k2/") && !K2_DOC_DIRS.iter().any(|d| lower.starts_with(d))
}

/// `n` bytes for a person: `512 B`, `3.0 KiB`, `40.0 MiB`.
pub fn human_bytes(n: u64) -> String {
    match n {
        n if n < 1 << 10 => format!("{n} B"),
        n if n < 1 << 20 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{:.1} MiB", n as f64 / 1_048_576.0),
    }
}

/// A file over the any-file cap is declined (exit 4), saying both sizes:
/// it can't be a tier-0 doc, nor the tier-0 fallback of a text doc whose
/// pack (with its own, smaller cap) refuses it.
pub fn size_cap(rel: &str, len: u64, cap: u64) -> Result<(), Refusal> {
    if len > cap {
        return Err(Refusal::declined(
            "too_large",
            format!(
                "{rel} is {}; files over {} can't be live docs (any-file docs keep every version)",
                human_bytes(len),
                human_bytes(cap)
            ),
        ));
    }
    Ok(())
}

/// The deny list for an existing, jailed file (`rel` workspace-relative,
/// `link` the path before symlinks were resolved, `abs` after). Applies to
/// every tier.
pub fn deny(rel: &str, link: &Path, abs: &Path) -> Result<(), Refusal> {
    if is_vault(rel) {
        return Err(Refusal::declined("protected", format!("{rel} is in the vault folder; it is never a live doc")));
    }
    if is_git_internal(rel) {
        return Err(Refusal::declined("protected", format!("{rel} is inside git's own folder; it is never a live doc")));
    }
    if Tier::of(rel) != Tier::Markdown && is_k2_config(rel) {
        return Err(Refusal::declined(
            "protected",
            format!("{rel} is in the workspace's private .k2 folder, where only Markdown and files under .k2/wiki, .k2/prds, .k2/docs or .k2/conflicts are live docs"),
        ));
    }
    let link_name = link.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_root_mirror = MIRRORS.contains(&link_name.as_str())
        && std::fs::symlink_metadata(link).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if is_generated(rel) || is_root_mirror {
        return Err(Refusal::declined(
            "generated",
            format!("K2 writes {rel} itself, so it can't be a live doc; edit its source instead"),
        ));
    }
    if let Some(hit) = k2_core::secret_paths::classify(rel, abs) {
        return Err(Refusal::declined(
            "secret",
            format!("{rel} looks like it holds a secret ({}); it is never a live doc", hit.as_str()),
        ));
    }
    Ok(())
}

/// A registered workspace: `(project id, root path)`.
fn workspace_by_token(token: &str) -> Result<(String, String), Refusal> {
    let path = crate::workspace_msg::resolve_workspace_allowing_handle(token.trim())
        .ok_or_else(|| Refusal::not_found("workspace_not_found", format!("no workspace '{}' on this server", token.trim())))?;
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    }
    .ok_or_else(|| Refusal::not_found("workspace_not_found", format!("no workspace '{}' on this server", token.trim())))?;
    Ok((id, path))
}

fn workspace_by_id(id: &str) -> Result<(String, String), Refusal> {
    let path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, id).ok().map(|p| p.path)
    }
    .ok_or_else(|| Refusal::not_found("workspace_not_found", "your workspace is no longer registered"))?;
    Ok((id.to_string(), path))
}

/// The registered workspace whose root holds the canonical `abs` (longest
/// root wins: a workspace nested inside another owns its own files).
pub(super) fn workspace_holding(abs: &Path) -> Result<(String, PathBuf), Refusal> {
    let projects = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::list(&conn).unwrap_or_default()
    };
    projects
        .into_iter()
        .filter_map(|p| Path::new(&p.path).canonicalize().ok().map(|root| (p.id, root)))
        .filter(|(_, root)| abs.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .ok_or_else(|| Refusal::not_found("workspace_not_found", "that file is not inside a workspace on this server"))
}

fn rel_string(root: &Path, abs: &Path) -> Result<String, Refusal> {
    let rel = abs
        .strip_prefix(root)
        .map_err(|_| Refusal::bad("path_escapes", "path escapes the workspace"))?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    if parts.is_empty() {
        return Err(Refusal::bad("not_a_file", "name a file, not the workspace root"));
    }
    Ok(parts.join("/"))
}

/// Where `mv` puts a doc: `to` relative to the doc's workspace (or absolute
/// inside it). The new name must not exist yet, its folder must, it stays
/// in the jail, it keeps the doc's kind (Markdown stays Markdown, text stays
/// text, any other file stays any-file), and it passes the deny list by path (not the
/// vault, not generated, not a secret's name). Returns `(rel, abs)`.
pub fn resolve_new(t: &Target, to: &str) -> Result<(String, PathBuf), Refusal> {
    let to = to.trim();
    if to.is_empty() || to.contains('\0') {
        return Err(Refusal::bad("bad_request", "to is required: the new name"));
    }
    let rel_in = if Path::new(to).is_absolute() {
        let parent = Path::new(to)
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .ok_or_else(|| Refusal::not_found("folder_not_found", format!("no folder for {to}")))?;
        let name = Path::new(to).file_name().ok_or_else(|| Refusal::bad("bad_request", "to names no file"))?;
        let abs = parent.join(name);
        rel_string(&t.root, &abs).map_err(|_| Refusal::bad("other_workspace", "mv stays inside the doc's workspace"))?
    } else {
        to.to_string()
    };
    let root = t.root.to_string_lossy().to_string();
    let abs = crate::fs_routes::jail_rel_path(&root, &rel_in, true).map_err(|e| Refusal::bad("path_escapes", e))?;
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(Refusal { status: "409 Conflict", code: "exists", message: format!("{rel_in} already exists") });
    }
    if !abs.parent().is_some_and(Path::is_dir) {
        return Err(Refusal::not_found("folder_not_found", format!("no folder for {rel_in}")));
    }
    let parent = abs.parent().and_then(|p| p.canonicalize().ok()).ok_or_else(|| Refusal::bad("path_escapes", "bad folder"))?;
    let abs = parent.join(abs.file_name().unwrap_or_default());
    let rel = rel_string(&t.root, &abs)?;
    if is_board_file(&rel) {
        return Err(Refusal::board_file(&rel));
    }
    if !tier_now(&rel).same_kind(t.tier) {
        return Err(Refusal::bad(
            "tier_change",
            match t.tier {
                Tier::Markdown => format!("{} is a Markdown doc; mv keeps it one (name it .md or .markdown)", t.rel),
                Tier::Text(_) => format!("{} is a text doc; mv keeps it text ({rel} isn't: Markdown, or not text)", t.rel),
                Tier::Deck => format!("{} is a deck; mv keeps it one (name it .pptx)", t.rel),
                Tier::Timeline => format!("{} is a live timeline; mv keeps it one (name it .otio)", t.rel),
                Tier::Scene => format!("{} is a live scene; mv keeps it one (name it .blend)", t.rel),
                Tier::Blob => format!("{} is an any-file doc; mv keeps it one ({rel} would be a text doc)", t.rel),
            },
        ));
    }
    if is_vault(&rel) {
        return Err(Refusal::declined("protected", format!("{rel} is in the vault folder")));
    }
    if is_generated(&rel) {
        return Err(Refusal::declined("generated", format!("K2 writes {rel} itself")));
    }
    if is_git_internal(&rel) || (t.tier != Tier::Markdown && is_k2_config(&rel)) {
        return Err(Refusal::declined("protected", format!("{rel} is protected (git's folder or K2's configuration)")));
    }
    if k2_core::secret_paths::is_secret_rel(&rel) {
        return Err(Refusal::declined("secret", format!("{rel} is a secret's name; it is never a live doc")));
    }
    Ok((rel, abs))
}

/// A workspace by handle, name, id or root: `(project id, root path)`.
pub fn workspace_root(token: &str) -> Result<(String, String), Refusal> {
    workspace_by_token(token)
}

/// The workspace-relative path a share names (MI25): `""` or `.` is the
/// whole workspace (a folder share). With `must_exist`, the path must be an
/// existing folder, or a file that could be a live doc (the deny list
/// applies), jailed like any doc; a folder inside `.k2/vault` is refused.
/// Without it (unshare), the path is only normalised. Returns
/// `(rel, is_folder)`.
pub fn share_rel(root: &str, raw: &str, must_exist: bool) -> Result<(String, bool), Refusal> {
    let trimmed = raw.trim().trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok((String::new(), true));
    }
    if Path::new(trimmed).is_absolute() || trimmed.split('/').any(|s| s == "..") || trimmed.contains('\0') {
        return Err(Refusal::bad("bad_request", "path must be relative to the workspace, without '..'"));
    }
    if !must_exist {
        let rel = trimmed.split('/').filter(|s| !s.is_empty() && *s != ".").collect::<Vec<_>>().join("/");
        return Ok((rel, false));
    }
    let abs = crate::fs_routes::jail_rel_path(root, trimmed, false).map_err(|e| {
        if e.starts_with("Invalid path") {
            Refusal::not_found("file_not_found", format!("no file or folder {trimmed} in that workspace"))
        } else {
            Refusal::bad("path_escapes", e)
        }
    })?;
    // dunce, as jail_rel_path canonicalized `abs`: on Windows std's form is
    // `\\?\D:\…` and would never be a prefix of jail's `D:\…`.
    let root_canon = dunce::canonicalize(Path::new(root))
        .map_err(|e| Refusal::not_found("workspace_not_found", format!("workspace root unavailable: {e}")))?;
    if abs == root_canon {
        return Ok((String::new(), true));
    }
    let rel = rel_string(&root_canon, &abs)?;
    if abs.is_dir() {
        if is_vault(&rel) {
            return Err(Refusal::declined("protected", "the vault folder is never shared"));
        }
        return Ok((rel, true));
    }
    deny(&rel, &root_canon.join(trimmed), &abs)?;
    Ok((rel, false))
}

/// Resolve `path` (relative to `workspace`, or absolute) to a jailed,
/// allowed doc. `default_workspace` is the caller's own project id (agents),
/// used when `workspace` is empty.
pub fn resolve(workspace: Option<&str>, path: &str, default_workspace: Option<&str>) -> Result<Target, Refusal> {
    let t = resolve_any(workspace, path, default_workspace)?;
    let meta = std::fs::metadata(&t.abs).map_err(|_| Refusal::not_found("file_not_found", format!("no file {path}")))?;
    if is_board_file(&t.rel) {
        return Err(Refusal::board_file(&t.rel));
    }
    size_cap(&t.rel, meta.len(), super::blob_max_bytes())?;
    Ok(t)
}

/// The doc K2 serves at the real path `file`, when it serves one there: what
/// [`resolve`] makes of it as an absolute path (inside a registered
/// workspace, past the deny list, not a Board file, under the any-file cap),
/// as its key. `None` otherwise. The kit's `Host::document_at`.
pub fn doc_key_at(file: &Path) -> Option<DocKey> {
    resolve(None, file.to_str()?, None).ok().map(|t| t.key())
}

/// A Board file, for the board routes: the same jail, deny list and workspace
/// as a doc, and no size cap (the format allows 2 GiB, and its own limits apply).
pub fn resolve_board(workspace: Option<&str>, path: &str, default_workspace: Option<&str>) -> Result<Target, Refusal> {
    if !is_board_file(path.trim()) {
        return Err(Refusal::bad("not_a_board_file", format!("{} isn't a .k5board file", path.trim())));
    }
    let t = resolve_any(workspace, path, default_workspace)?;
    // A symlink named .k5board may point at anything: the file it reaches decides.
    if !is_board_file(&t.rel) {
        return Err(Refusal::bad("not_a_board_file", format!("{path} isn't a .k5board file")));
    }
    Ok(t)
}

/// The folder a new board goes in, as the `Target` of a file there (for the
/// rights check): an existing folder in a registered workspace or one of its
/// checkouts, and never one the deny list refuses.
pub fn board_folder(workspace: Option<&str>, dir: &str, default_workspace: Option<&str>) -> Result<Target, Refusal> {
    let dir = dir.trim();
    if dir.contains('\0') {
        return Err(Refusal::bad("bad_request", "dir contains a NUL byte"));
    }
    let not_workspace = || Refusal::not_found("not_a_workspace", format!("{dir} isn't a folder in a workspace on this server"));
    let (project_id, root, abs_dir) = if Path::new(dir).is_absolute() {
        let abs = Path::new(dir).canonicalize().map_err(|_| not_workspace())?;
        let (id, root) = workspace_holding(&abs).map_err(|_| not_workspace())?;
        (id, root, abs)
    } else {
        let (id, root_str) = match workspace.map(str::trim).filter(|w| !w.is_empty()) {
            Some(ws) => workspace_by_token(ws)?,
            None => match default_workspace {
                Some(own) => workspace_by_id(own)?,
                None => return Err(Refusal::bad("workspace_required", "name the workspace (workspace=<name>) or give an absolute path")),
            },
        };
        let root = Path::new(&root_str).canonicalize().map_err(|_| not_workspace())?;
        let abs = root.join(dir).canonicalize().map_err(|_| not_workspace())?;
        (id, root, abs)
    };
    if !abs_dir.is_dir() || !abs_dir.starts_with(&root) {
        return Err(not_workspace());
    }
    let abs = abs_dir.join("Untitled.k5board");
    let rel = rel_string(&root, &abs)?;
    deny(&rel, &abs, &abs)?;
    Ok(Target { project_id, root, rel, abs, tier: Tier::Blob })
}

fn resolve_any(workspace: Option<&str>, path: &str, default_workspace: Option<&str>) -> Result<Target, Refusal> {
    let path = path.trim();
    if path.is_empty() {
        return Err(Refusal::bad("bad_request", "path is required"));
    }
    if path.contains('\0') {
        return Err(Refusal::bad("bad_request", "path contains a NUL byte"));
    }
    let (project_id, root, link) = if Path::new(path).is_absolute() {
        let link = PathBuf::from(path);
        let abs = link
            .canonicalize()
            .map_err(|_| Refusal::not_found("file_not_found", format!("no file at {path}")))?;
        let (id, root) = workspace_holding(&abs)?;
        if let Some(ws) = workspace.map(str::trim).filter(|w| !w.is_empty()) {
            let (named, _) = workspace_by_token(ws)?;
            if named != id {
                return Err(Refusal::bad("workspace_mismatch", format!("{path} is not in workspace '{ws}'")));
            }
        }
        (id, root, link)
    } else {
        let (id, root_str) = match workspace.map(str::trim).filter(|w| !w.is_empty()) {
            Some(ws) => workspace_by_token(ws)?,
            None => match default_workspace {
                Some(own) => workspace_by_id(own)?,
                None => return Err(Refusal::bad("workspace_required", "name the workspace (workspace=<name>) or give an absolute path")),
            },
        };
        // The jail: relative only, no `..`, and after symlinks still inside
        // the root (the checks below repeat the last one for both forms).
        crate::fs_routes::jail_rel_path(&root_str, path, false).map_err(|e| {
            if e.starts_with("Invalid path") {
                Refusal::not_found("file_not_found", format!("no file {path} in that workspace"))
            } else {
                Refusal::bad("path_escapes", e)
            }
        })?;
        let root = Path::new(&root_str)
            .canonicalize()
            .map_err(|e| Refusal::not_found("workspace_not_found", format!("workspace root unavailable: {e}")))?;
        let link = root.join(path);
        (id, root, link)
    };
    let abs = link
        .canonicalize()
        .map_err(|_| Refusal::not_found("file_not_found", format!("no file {path}")))?;
    if !abs.starts_with(&root) {
        return Err(Refusal::bad("path_escapes", "path escapes the workspace"));
    }
    let meta = std::fs::metadata(&abs).map_err(|_| Refusal::not_found("file_not_found", format!("no file {path}")))?;
    if !meta.is_file() {
        return Err(Refusal::bad("not_a_file", format!("{path} is not a file")));
    }
    let rel = rel_string(&root, &abs)?;
    deny(&rel, &link, &abs)?;
    let tier = tier_now(&rel);
    Ok(Target { project_id, root, rel, abs, tier })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_paths_match_by_segment() {
        for p in [
            ".k2/AGENTS.md",
            ".k2/skills/k2-cli/SKILL.md",
            ".k2/context/catalog/team-roster.md",
            ".claude/skills/anything/SKILL.md",
            ".opencode/agent/x.md",
        ] {
            assert!(is_generated(p), "{p}");
        }
        for p in [
            ".k2/wiki/Home.md",
            ".k2/skills/my-own/SKILL.md",
            ".k2/context/catalog/notes.md",
            "AGENTS.md",
            ".claude/skills/a/b/SKILL.md",
        ] {
            assert!(!is_generated(p), "{p}");
        }
    }

    #[test]
    fn vault_is_protected() {
        assert!(is_vault(".k2/vault"));
        assert!(is_vault(".k2/VAULT/x.md"));
        assert!(!is_vault(".k2/vaulted.md"));
    }

    /// The private folder has had three names; a leftover `.k2so/` can sit
    /// beside `.k2/` with root links still pointing into it. Each rule
    /// that says `.k2/…` holds under every name and spelling.
    #[test]
    fn the_k2_rules_hold_under_each_name_of_the_folder() {
        for dir in [".k2", ".k2so", ".k5", ".K2", ".K2SO", ".K5", ".k2so.", ".k5 "] {
            assert!(is_vault(&format!("{dir}/vault")), "{dir}");
            assert!(is_vault(&format!("{dir}/vault/vault.db")), "{dir}");
            assert!(is_vault(&format!("{dir}/VAULT/x.md")), "{dir}");
            assert!(!is_vault(&format!("{dir}/vaulted.md")), "{dir}");

            for p in ["settings.json", "heartbeats/digest/WAKEUP.md", "agent/ROLE.md", "hooks/notify.sh", "inbox/a.txt"] {
                assert!(is_k2_config(&format!("{dir}/{p}")), "{dir}/{p}");
            }
            for p in ["wiki/Home.md", "wiki/img/a.png", "prds/plan.md", "docs/a.pdf", "conflicts/a.md"] {
                assert!(!is_k2_config(&format!("{dir}/{p}")), "{dir}/{p} is a doc folder");
            }

            for p in ["AGENTS.md", "skills/k2-cli/SKILL.md", "skills/k2-zen/SKILL.md", "context/catalog/team-roster.md"] {
                assert!(is_generated(&format!("{dir}/{p}")), "{dir}/{p}");
            }
            for p in ["wiki/Home.md", "skills/my-own/SKILL.md", "context/catalog/notes.md"] {
                assert!(!is_generated(&format!("{dir}/{p}")), "{dir}/{p}");
            }
        }
        // Only the workspace's own folder, and only that name.
        for p in [
            "docs/.k2so/vault/x",
            ".k2so-old/vault/x",
            ".k5x/vault/x",
            "k2so/vault/x",
            "notes/.k5/settings.json",
        ] {
            assert!(!is_vault(p), "{p}");
            assert!(!is_k2_config(p), "{p}");
            assert!(!is_generated(p), "{p}");
        }
        assert_eq!(with_k2_dot_dir(".K2SO/Wiki/Home.md"), ".k2/wiki/home.md");
        assert_eq!(with_k2_dot_dir(".k5"), ".k2");
        assert_eq!(with_k2_dot_dir("Notes/.k2so/x"), "notes/.k2so/x");
        assert_eq!(with_k2_dot_dir(""), "");
    }

    #[test]
    fn a_board_file_is_blob_by_any_spelling_and_nothing_else_is() {
        for name in ["plan.k5board", "a/b/plan.k5board", "X.K5BOARD", "Makefile.k5board", ".env.k5board", ".k5board", "a.md.k5board"] {
            assert!(is_board_file(name), "{name}");
            assert_eq!(Tier::of(name), Tier::Blob, "{name}");
        }
        for name in ["plan.k5board.md", "k5board", "plan.k5boardx", "plank5board", "é.md"] {
            assert!(!is_board_file(name), "{name}");
        }
        assert_eq!(Tier::of("plan.k5board.md"), Tier::Markdown);
    }

    /// A `.pptx` (any case) is a deck by name; nothing else is. A deck is a
    /// live doc (tier 1, the pptx pack), and mv keeps it one.
    #[test]
    fn a_pptx_is_a_deck_by_name_and_nothing_else_is() {
        for name in ["talk.pptx", "a/b/Talk.PPTX", "q3 review.Pptx", ".hidden.pptx"] {
            assert!(is_deck_file(name), "{name}");
            assert_eq!(Tier::of(name), Tier::Deck, "{name}");
        }
        for name in [".pptx", "talk.ppt", "talk.pptx.zip", "talk.potx", "talk.ppsx", "pptx", "dir.pptx/a.md"] {
            assert!(!is_deck_file(name), "{name}");
            assert_ne!(Tier::of(name), Tier::Deck, "{name}");
        }
        assert_eq!((Tier::Deck.number(), Tier::Deck.pack(), Tier::Deck.family()), (1, "pptx", "deck"));
        assert!(Tier::Deck.same_kind(Tier::Deck));
        assert!(!Tier::Deck.same_kind(Tier::Blob) && !Tier::Blob.same_kind(Tier::Deck));
    }

    /// An `.otio` is a timeline and a `.blend` a scene by name: live keyed
    /// docs (tier 1, their packs' schema names), and mv keeps each one. The
    /// other files the timeline and fab cores read are any-file docs by
    /// name, never live text, though the text pack would take most of them.
    #[test]
    fn timelines_and_scenes_by_name_and_the_other_pack_files_are_any_file() {
        for name in ["cut.otio", "a/b/Cut.OTIO", ".x.otio"] {
            assert!(is_timeline_file(name), "{name}");
            assert_eq!(Tier::of(name), Tier::Timeline, "{name}");
        }
        for name in ["scene.blend", "a/Room.BLEND"] {
            assert!(is_scene_file(name), "{name}");
            assert_eq!(Tier::of(name), Tier::Scene, "{name}");
        }
        for name in [".otio", "cut.otio.json", "scene.blend1", "otio", "dir.otio/a.md"] {
            assert!(!is_timeline_file(name) && !is_scene_file(name), "{name}");
            assert!(!matches!(Tier::of(name), Tier::Timeline | Tier::Scene), "{name}");
        }
        for name in ["edit/Info.fcpxml", "Show.prproj", "board.gbr", "board.GTL", "board.ger", "board.drl", "board.xnc", "board.xln", "board.ncd", "board.gko"] {
            assert_eq!(Tier::of(name), Tier::Blob, "{name}");
        }
        assert!(k2_mpc_text::TextPack::for_path("board.gbr").is_some(), "the text pack would take a photoplot file as text");
        assert_eq!((Tier::Timeline.number(), Tier::Timeline.pack(), Tier::Timeline.family()), (1, "timeline-otio", "timeline"));
        assert_eq!((Tier::Scene.number(), Tier::Scene.pack(), Tier::Scene.family()), (1, "blend", "scene"));
        assert_eq!(Tier::Deck.pack(), k2_mpc_pptx::live::SCHEMA.name, "a keyed tier's pack is its schema's name");
        for t in [Tier::Deck, Tier::Timeline, Tier::Scene] {
            assert!(t.is_keyed() && t.same_kind(t), "{t:?}");
            assert!(!t.same_kind(Tier::Blob) && !Tier::Blob.same_kind(t), "{t:?}");
        }
        assert!(!Tier::Timeline.same_kind(Tier::Scene) && !Tier::Deck.same_kind(Tier::Timeline));
        assert!(!Tier::Blob.is_keyed() && !Tier::Markdown.is_keyed());
    }

    /// While its core is off, a live keyed doc is an any-file doc; on, its
    /// own tier.
    #[test]
    fn a_timeline_or_scene_is_any_file_while_its_core_is_off() {
        let _l = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        assert_eq!(tier_now("cut.otio"), Tier::Blob);
        assert_eq!(tier_now("room.blend"), Tier::Blob);
        super::super::cores::set_for_test("timeline", true);
        assert_eq!(tier_now("cut.otio"), Tier::Timeline);
        assert_eq!(tier_now("room.blend"), Tier::Blob, "the blend core is its own");
        super::super::cores::set_for_test("blend", true);
        assert_eq!(tier_now("room.blend"), Tier::Scene);
        assert_eq!(tier_now("edit.fcpxml"), Tier::Blob, "an .fcpxml is tier 0, on or off");
        super::super::cores::tests::all_off();
    }

    #[test]
    fn a_share_can_name_a_board_file_and_the_deny_list_stays_about_paths() {
        let d = tmp();
        let p = d.join("plan.k5board");
        std::fs::write(&p, b"PK").expect("write");
        assert_eq!(deny("plan.k5board", &p, &p), Ok(()));
        let root = d.to_string_lossy().to_string();
        assert_eq!(share_rel(&root, "plan.k5board", true).expect("a share can name one"), ("plan.k5board".to_string(), false));
    }

    #[test]
    fn a_doc_cant_be_moved_onto_a_board_name() {
        let d = tmp();
        std::fs::write(d.join("a.bin"), b"x").expect("write");
        let t = Target { project_id: "p".into(), root: d.clone(), rel: "a.bin".into(), abs: d.join("a.bin"), tier: Tier::Blob };
        let e = resolve_new(&t, "a.k5board").expect_err("a Blob-tier file named as a board");
        assert_eq!(e.code, "board_file");
        assert_eq!(e.status, "422 Unprocessable Entity");
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-docs-target-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("mkdir");
        d.canonicalize().expect("canon")
    }

    #[test]
    fn deny_list_refuses_each_kind_and_admits_markdown_and_any_other_file() {
        let d = tmp();
        let w = |rel: &str, body: &[u8]| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(&p, body).expect("write");
            p
        };
        let ok = w("notes/a.md", b"# hi\n");
        assert_eq!(deny("notes/a.md", &ok, &ok), Ok(()));
        let txt = w("a.txt", b"x");
        assert_eq!(deny("a.txt", &txt, &txt), Ok(()));
        let png = w("img/logo.png", &[0x89, b'P', b'N', b'G', 0, 0xff]);
        assert_eq!(deny("img/logo.png", &png, &png), Ok(()));
        let v = w(".k2/vault/k.md", b"x");
        assert_eq!(deny(".k2/vault/k.md", &v, &v).expect_err("vault").code, "protected");
        let vb = w(".k2/vault/blob.bin", &[0, 1, 2]);
        assert_eq!(deny(".k2/vault/blob.bin", &vb, &vb).expect_err("vault blob").code, "protected");
        let g = w(".k2/AGENTS.md", b"x");
        assert_eq!(deny(".k2/AGENTS.md", &g, &g).expect_err("gen").code, "generated");
        let s = w(".ssh/notes.md", b"x");
        assert_eq!(deny(".ssh/notes.md", &s, &s).expect_err("secret").code, "secret");
        let key = w("keys/deploy.pem", b"x");
        assert_eq!(deny("keys/deploy.pem", &key, &key).expect_err("secret by name").code, "secret");
        let env = w(".env", b"API_KEY=sk-live-0123456789abcdef0123456789abcdef\n");
        assert_eq!(deny(".env", &env, &env).expect_err("dotenv").code, "secret");
        let hook = w(".git/hooks/pre-commit", b"#!/bin/sh\n");
        assert_eq!(deny(".git/hooks/pre-commit", &hook, &hook).expect_err("git hook").code, "protected");
        let cfg = w(".k2/heartbeats.json", b"{}");
        assert_eq!(deny(".k2/heartbeats.json", &cfg, &cfg).expect_err("k2 config").code, "protected");
        let att = w(".k2/wiki/assets/map.png", &[1, 2, 3]);
        assert_eq!(deny(".k2/wiki/assets/map.png", &att, &att), Ok(()));
        // The same under a leftover `.k2so/` and the coming `.k5/`. The
        // generated skill under `.k2so/` is what a stale root link
        // (CLAUDE.md) may still point at.
        for dir in [".k2so", ".K2SO", ".k5"] {
            assert_eq!(deny(&format!("{dir}/vault/k.md"), &v, &v).expect_err("vault").code, "protected", "{dir}");
            assert_eq!(deny(&format!("{dir}/heartbeats.json"), &cfg, &cfg).expect_err("config").code, "protected", "{dir}");
            assert_eq!(deny(&format!("{dir}/skills/k2-cli/SKILL.md"), &g, &g).expect_err("gen").code, "generated", "{dir}");
            assert_eq!(deny(&format!("{dir}/AGENTS.md"), &g, &g).expect_err("gen").code, "generated", "{dir}");
            assert_eq!(deny(&format!("{dir}/wiki/assets/map.png"), &att, &att), Ok(()), "{dir}: a doc folder");
        }
        let prd = w(".k2/PROJECT.md", b"# p\n");
        assert_eq!(deny(".k2/PROJECT.md", &prd, &prd), Ok(()), "Markdown under .k2/ stays a doc");
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    /// Design, CAD and 3D files are any-file docs whatever the pack would
    /// guess; the pack's near neighbours stay text.
    #[test]
    fn design_cad_and_3d_files_are_any_file_docs() {
        for p in [
            "art/poster.psd", "art/big.PSB", "scene.blend1", "part.dwg", "plan.DXF", "gear.step", "gear.stp",
            "gear.iges", "site.ifc", "board.kicad_pcb", "bracket.sldprt", "body.f3d", "house.skp", "print.stl", "mesh.obj", "mesh.ply", "model.fbx",
            "model.glb", "model.gltf", "print.3mf", "stage.usdz", "sheet.kicad_sch", "lib.kicad_sym",
            "fp.kicad_mod", "part.p21", "site.ifczip",
        ] {
            assert_eq!(Tier::of(p), Tier::Blob, "{p}");
        }
        for p in ["profile.psd1", "step", "notes.step.txt", ".step", "obj"] {
            assert_ne!(Tier::of(p), Tier::Blob, "{p}");
        }
        assert!(!is_design_file("a/.dwg"), "a dotfile named .dwg has no extension");
    }

    /// K2's list doesn't take a type away from the pack's table: each entry
    /// is a name the pack doesn't know (`Unknown`), or one it already calls
    /// not text (`NotText`, text v0.2.22 on; redundant here, never live), never
    /// a text type of its own.
    #[test]
    fn the_design_list_takes_nothing_from_the_text_table() {
        for e in DESIGN_EXTENSIONS {
            assert_eq!(*e, e.to_ascii_lowercase(), "{e}");
            let p = format!("file.{e}");
            let l = k2_mpc_text::lookup(&p);
            assert!(
                matches!(l, k2_mpc_text::Lookup::Unknown | k2_mpc_text::Lookup::NotText),
                "{e} is a text type in the pack's own table now ({l:?}): drop it here"
            );
        }
    }

    /// Text v0.2.22: CAD, BIM and circuit-board files are on the pack's
    /// not-text list (their lines point at each other by id). Every one is a
    /// tier-0 doc, never live text, by any spelling, whether or not K2's own
    /// list names it.
    #[test]
    fn cad_bim_and_circuit_board_files_are_tier_0_never_live_text() {
        for e in [
            "ifc", "ifczip", "step", "stp", "p21", "dxf", "kicad_sch", "kicad_pcb", "kicad_sym", "kicad_mod", "iges", "igs", "obj",
        ] {
            assert_eq!(k2_mpc_text::lookup(&format!("f.{e}")), k2_mpc_text::Lookup::NotText, "{e}: the pack's not-text list");
            for p in [format!("model.{e}"), format!("cad/Part.{}", e.to_ascii_uppercase())] {
                assert_eq!(Tier::of(&p), Tier::Blob, "{p}");
                assert!(k2_mpc_text::TextPack::for_path(&p).is_none(), "{p}: no text pack opens it");
            }
        }
        // G-code and ASCII STL have no ids between lines: G-code stays live
        // text (STL is a 3D model on K2's own list, tier 0 either way).
        assert!(matches!(Tier::of("part.gcode"), Tier::Text(_)));
    }

    #[test]
    fn markdown_first_then_text_by_family_then_any_file() {
        for p in ["a.md", "notes/b.markdown", "C.MD"] {
            assert_eq!(Tier::of(p), Tier::Markdown, "{p}");
        }
        use k2_mpc_text::Family as F;
        for (p, f) in [
            ("a.txt", F::Text),
            ("server.log", F::Text),
            ("README", F::Text),
            ("main.rs", F::Code),
            ("Makefile", F::Code),
            ("app.tsx", F::Code),
            ("config.yaml", F::Config),
            ("Cargo.toml", F::Config),
            ("page.html", F::Markup),
            ("flow.mmd", F::Diagram),
            ("data.csv", F::CsvAsText),
            ("data.tsv", F::CsvAsText),
        ] {
            assert_eq!(Tier::of(p), Tier::Text(f), "{p}");
            assert_eq!(Tier::of(p).number(), 1, "{p}");
        }
        for p in ["logo.png", "report.pdf", "deck.ppt", "song.mp3", "a.zip", "font.woff2"] {
            assert_eq!(Tier::of(p), Tier::Blob, "{p}");
        }
        // A .pptx is a deck by name (the Slides core; a tier-0 doc while it is off).
        assert_eq!(Tier::of("deck.pptx"), Tier::Deck);
        assert_eq!(Tier::Blob.number(), 0);
        assert_eq!(Tier::of("data.csv").family(), "csv-as-text");
        assert!(Tier::of("a.txt").same_kind(Tier::of("b.toml")), "text stays text across families");
        assert!(!Tier::of("a.txt").same_kind(Tier::Markdown));
        assert!(!Tier::of("a.txt").same_kind(Tier::Blob));
    }

    #[test]
    fn a_tier_0_file_over_the_cap_is_declined_with_both_sizes() {
        let r = size_cap("big.bin", 40 << 20, 32 << 20).expect_err("over the cap");
        assert_eq!((r.status, r.code), ("422 Unprocessable Entity", "too_large"));
        assert!(r.message.contains("40.0 MiB") && r.message.contains("32.0 MiB"), "{}", r.message);
        assert_eq!(size_cap("big.bin", 32 << 20, 32 << 20), Ok(()), "the cap itself is allowed");
        assert!(size_cap("big.md", 40 << 20, 32 << 20).is_err(), "too big for its tier-0 fallback too");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 << 10), "3.0 KiB");
    }

    #[cfg(unix)]
    #[test]
    fn a_root_mirror_symlink_is_generated_but_a_real_claude_md_is_not() {
        let d = tmp();
        std::fs::create_dir_all(d.join(".k2")).expect("mkdir");
        std::fs::write(d.join(".k2/PROJECT.md"), "p").expect("write");
        std::os::unix::fs::symlink(d.join(".k2/PROJECT.md"), d.join("CLAUDE.md")).expect("symlink");
        let abs = d.join("CLAUDE.md").canonicalize().expect("canon");
        assert_eq!(deny(".k2/PROJECT.md", &d.join("CLAUDE.md"), &abs).expect_err("mirror").code, "generated");
        let own = d.join("GEMINI.md");
        std::fs::write(&own, "mine").expect("write");
        assert_eq!(deny("GEMINI.md", &own, &own), Ok(()));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }
}
