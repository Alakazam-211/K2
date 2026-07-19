//! Per-workspace knowledge base (brain map) under `<workspace>/.k2/wiki/`.
//!
//! Walks Markdown notes, extracts YAML frontmatter + `[[wikilinks]]`, and
//! builds a graph index for the in-app map and optional localhost site.
//! See `.k2/prds/prd-workspace-kb-brain-map-and-publish.md`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Relative path of the wiki directory under a workspace root.
pub const WIKI_REL: &str = ".k2/wiki";

/// Host-level **wiki-index** under `~/.k2/wiki/` — registry of workspace
/// brains only. Never injected into per-workspace indexes.
///
/// Files:
/// - `_workspaces.json` — machine registry (source of truth for the K2 tab)
/// - `_Index.md` — human-readable list of workspaces (not linked from brains)
pub const HOST_WIKI_REL: &str = "wiki";
/// Canonical name for the host-level registry product surface ("wiki-index").
pub const HOST_WIKI_INDEX_NAME: &str = "wiki-index";

/// Max note body size returned by the note reader (1 MiB).
pub const MAX_NOTE_BYTES: u64 = 1024 * 1024;

/// Separator for fleet node ids: `{workspaceId}::{noteId}`.
pub const FLEET_ID_SEP: &str = "::";

/// Synthetic fleet node id prefix for focus-group hubs: `__focusgroup__::{groupId}`.
pub const FOCUS_GROUP_NODE_PREFIX: &str = "__focusgroup__::";
/// Synthetic fleet node id prefix for project hubs: `__project__::{projectId}`.
pub const PROJECT_NODE_PREFIX: &str = "__project__::";

/// Link kinds on the fleet map (workspace indexes leave `kind` unset / wikilink).
pub const LINK_KIND_WIKILINK: &str = "wikilink";
pub const LINK_KIND_WORKSPACE_HUB: &str = "workspaceHub";
pub const LINK_KIND_FOCUS_GROUP: &str = "focusGroup";
pub const LINK_KIND_PROJECT: &str = "project";

/// Node kinds — synthetic hubs vs real notes.
pub const NODE_KIND_NOTE: &str = "note";
pub const NODE_KIND_WORKSPACE_HUB: &str = "workspaceHub";
pub const NODE_KIND_FOCUS_GROUP: &str = "focusGroup";
pub const NODE_KIND_PROJECT: &str = "project";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WikiNode {
    /// Stable id: wiki-rel path (e.g. `Home.md`), or fleet id `wsId::Home.md`.
    pub id: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    /// Path relative to workspace root (e.g. `.k2/wiki/Home.md`).
    pub path: String,
    pub exists: bool,
    /// Set on fleet (K2-tab) maps — which workspace brain owns this note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    /// `"note"` | `"workspaceHub"` | `"focusGroup"` | `"project"` (fleet map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Focus group membership (fleet map, when focus groups are enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_group_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_group_color: Option<String>,
    /// Project V1 membership on synthetic project hubs (and for filters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WikiLink {
    pub source: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// `"wikilink"` | `"workspaceHub"` | `"focusGroup"` | `"project"`. Unset = wikilink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Focus group present on the K2 fleet index (when enabled).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WikiFocusGroup {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Workspace project ids that have a wiki and belong to this group.
    pub workspace_ids: Vec<String>,
}

/// Project (V1 project group) on the K2 fleet Projects map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WikiProject {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Member workspace ids that have a wiki on this host.
    pub workspace_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WikiIndex {
    pub workspace_path: String,
    pub wiki_rel: String,
    pub generated_at: String,
    pub nodes: Vec<WikiNode>,
    pub links: Vec<WikiLink>,
    pub note_count: usize,
    /// `"workspace"` (default) or `"k2"` fleet map across workspaces.
    #[serde(default = "default_scope")]
    pub scope: String,
    /// Focus groups included on the fleet map (empty when disabled / none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<WikiFocusGroup>,
    /// Projects (V1) on the fleet map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<WikiProject>,
}

fn default_scope() -> String {
    "workspace".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostWorkspaceEntry {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostWorkspaceRegistry {
    pub updated_at: String,
    pub workspaces: Vec<HostWorkspaceEntry>,
}

/// Resolve `<workspace>/.k2/wiki`, confined under the workspace root.
pub fn wiki_root(workspace: &Path) -> Result<PathBuf, String> {
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("workspace path: {e}"))?;
    let wiki = root.join(WIKI_REL);
    if wiki.exists() {
        let canon = wiki
            .canonicalize()
            .map_err(|e| format!("wiki path: {e}"))?;
        if !canon.starts_with(&root) {
            return Err("wiki path escapes workspace root".into());
        }
        Ok(canon)
    } else {
        // Not created yet — still confined under root.
        Ok(wiki)
    }
}

/// Build a full graph index for the workspace wiki (empty if missing).
pub fn build_index(workspace: &Path) -> Result<WikiIndex, String> {
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("workspace path: {e}"))?;
    let wiki = root.join(WIKI_REL);
    let generated_at = chrono_now();

    if !wiki.is_dir() {
        return Ok(WikiIndex {
            workspace_path: root.to_string_lossy().into_owned(),
            wiki_rel: WIKI_REL.into(),
            generated_at,
            nodes: vec![],
            links: vec![],
            note_count: 0,
            scope: "workspace".into(),
            groups: vec![],
            projects: vec![],
        });
    }

    let wiki_canon = wiki
        .canonicalize()
        .map_err(|e| format!("wiki path: {e}"))?;
    if !wiki_canon.starts_with(&root) {
        return Err("wiki path escapes workspace root".into());
    }

    let mut files: Vec<PathBuf> = Vec::new();
    walk_md(&wiki_canon, &mut files)?;

    // First pass: parse nodes + raw outbound link targets (unresolved titles).
    struct Partial {
        id: String,
        title: String,
        aliases: Vec<String>,
        tags: Vec<String>,
        path: String,
        out_titles: Vec<String>,
    }

    let mut partials: Vec<Partial> = Vec::new();
    for abs in &files {
        let rel_wiki = abs
            .strip_prefix(&wiki_canon)
            .map_err(|_| "path not under wiki".to_string())?;
        let id = rel_wiki
            .to_string_lossy()
            .replace('\\', "/");
        let rel_ws = Path::new(WIKI_REL).join(rel_wiki);
        let path = rel_ws.to_string_lossy().replace('\\', "/");

        let raw = fs::read_to_string(abs).map_err(|e| format!("read {}: {e}", abs.display()))?;
        let (fm, body) = split_frontmatter(&raw);
        let stem = abs
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.clone());
        let title = fm
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(stem);
        let aliases = fm.aliases.unwrap_or_default();
        let tags = fm.tags.unwrap_or_default();
        let out_titles = extract_wikilink_targets(&body);

        partials.push(Partial {
            id,
            title,
            aliases,
            tags,
            path,
            out_titles,
        });
    }

    // Lookup: lowercased key → node id
    let mut lookup: HashMap<String, String> = HashMap::new();
    for p in &partials {
        let id = p.id.clone();
        insert_lookup(&mut lookup, &p.title, &id);
        for a in &p.aliases {
            insert_lookup(&mut lookup, a, &id);
        }
        if let Some(stem) = Path::new(&p.id).file_stem() {
            insert_lookup(&mut lookup, &stem.to_string_lossy(), &id);
        }
        // also key by id / bare filename
        insert_lookup(&mut lookup, &p.id, &id);
        if let Some(name) = Path::new(&p.id).file_name() {
            insert_lookup(&mut lookup, &name.to_string_lossy(), &id);
        }
    }

    let mut links: Vec<WikiLink> = Vec::new();
    let mut missing_nodes: HashMap<String, WikiNode> = HashMap::new();

    for p in &partials {
        for title in &p.out_titles {
            let key = normalize_key(title);
            if let Some(target_id) = lookup.get(&key) {
                links.push(WikiLink {
                    source: p.id.clone(),
                    target: target_id.clone(),
                    missing: false,
                    kind: None,
                });
            } else {
                let miss_id = format!("missing:{}", title.trim());
                links.push(WikiLink {
                    source: p.id.clone(),
                    target: miss_id.clone(),
                    missing: true,
                    kind: None,
                });
                missing_nodes.entry(miss_id.clone()).or_insert(WikiNode {
                    id: miss_id,
                    title: title.trim().to_string(),
                    aliases: vec![],
                    tags: vec![],
                    path: String::new(),
                    exists: false,
                    workspace_id: None,
                    workspace_name: None,
                    workspace_path: None,
                    kind: None,
                    focus_group_id: None,
                    focus_group_name: None,
                    focus_group_color: None,
                    project_id: None,
                    project_name: None,
                    project_color: None,
                });
            }
        }
    }

    let mut nodes: Vec<WikiNode> = partials
        .into_iter()
        .map(|p| WikiNode {
            id: p.id,
            title: p.title,
            aliases: p.aliases,
            tags: p.tags,
            path: p.path,
            exists: true,
            workspace_id: None,
            workspace_name: None,
            workspace_path: None,
            kind: None,
            focus_group_id: None,
            focus_group_name: None,
            focus_group_color: None,
            project_id: None,
            project_name: None,
            project_color: None,
        })
        .collect();
    let note_count = nodes.len();
    nodes.extend(missing_nodes.into_values());
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    links.sort_by(|a, b| (&a.source, &a.target).cmp(&(&b.source, &b.target)));
    links.dedup_by(|a, b| a.source == b.source && a.target == b.target);

    Ok(WikiIndex {
        workspace_path: root.to_string_lossy().into_owned(),
        wiki_rel: WIKI_REL.into(),
        generated_at,
        nodes,
        links,
        note_count,
        scope: "workspace".into(),
        groups: vec![],
        projects: vec![],
    })
}

// ── Host (~/.k2/wiki) workspace registry + fleet map ───────────────────

/// `~/.k2/wiki` — host-level registry of workspace brains.
pub fn host_wiki_dir() -> PathBuf {
    crate::paths::k2_home().join(HOST_WIKI_REL)
}

pub fn fleet_node_id(workspace_id: &str, note_id: &str) -> String {
    format!("{workspace_id}{FLEET_ID_SEP}{note_id}")
}

/// Synthetic focus-group hub id on the fleet map.
pub fn focus_group_node_id(group_id: &str) -> String {
    format!("{FOCUS_GROUP_NODE_PREFIX}{group_id}")
}

/// Synthetic project hub id on the fleet map.
pub fn project_node_id(project_id: &str) -> String {
    format!("{PROJECT_NODE_PREFIX}{project_id}")
}

/// Split `wsId::noteId` — note ids may contain `::` rarely; split on first only.
/// Returns `None` for synthetic focus-group / project ids.
pub fn parse_fleet_node_id(id: &str) -> Option<(&str, &str)> {
    if id.starts_with(FOCUS_GROUP_NODE_PREFIX) || id.starts_with(PROJECT_NODE_PREFIX) {
        return None;
    }
    id.split_once(FLEET_ID_SEP)
}

/// Refresh `~/.k2/wiki/_workspaces.json` + human `_Index.md` from registered
/// projects that have a `.k2/wiki/` directory. **Does not** write into any
/// workspace wiki (workspaces stay unaware of the host index).
pub fn sync_host_workspace_registry() -> Result<HostWorkspaceRegistry, String> {
    let projects = crate::projects_ops::projects_list().unwrap_or_default();
    let mut workspaces = Vec::new();
    for p in projects {
        let path = PathBuf::from(&p.path);
        let wiki = path.join(WIKI_REL);
        if wiki.is_dir() {
            workspaces.push(HostWorkspaceEntry {
                id: p.id.clone(),
                name: p.name.clone(),
                path: p.path.clone(),
            });
        }
    }
    workspaces.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));

    let reg = HostWorkspaceRegistry {
        updated_at: chrono_now(),
        workspaces,
    };

    let host = host_wiki_dir();
    fs::create_dir_all(&host).map_err(|e| format!("mkdir host wiki: {e}"))?;
    let json_path = host.join("_workspaces.json");
    let json = serde_json::to_string_pretty(&reg).map_err(|e| e.to_string())?;
    fs::write(&json_path, format!("{json}\n")).map_err(|e| format!("write registry: {e}"))?;

    // Human-readable wiki-index — only at host level, never linked from workspaces.
    let mut md = String::from(
        "---\ntitle: wiki-index\naliases: [K2 Wiki Index, Host Wiki Index]\ntags: [k2, wiki-index, host-index]\n---\n\n# wiki-index — workspace brains\n\n",
    );
    md.push_str(
        "Host-level registry of workspaces that have a knowledge base under `.k2/wiki/`.\n\
         Machine form: `_workspaces.json`. Per-workspace wikis **do not** link here — registration is one-way.\n\n",
    );
    if reg.workspaces.is_empty() {
        md.push_str("_No workspace wikis found yet. Open a workspace → View Wiki → Seed wiki._\n");
    } else {
        for w in &reg.workspaces {
            md.push_str(&format!("- **{}** — `{}`\n", w.name, w.path));
        }
    }
    fs::write(host.join("_Index.md"), md).map_err(|e| format!("write host _Index: {e}"))?;

    Ok(reg)
}

/// Build a merged graph of every workspace brain listed in the host registry
/// (after syncing from projects). Node ids are `{workspaceId}::{noteId}`.
///
/// When focus groups are enabled, each group becomes a synthetic hub node
/// linked to member **workspace hubs** (`kind: focusGroup`) — not article
/// wikilinks. Per-workspace notes keep normal wikilink edges only.
pub fn build_fleet_index() -> Result<WikiIndex, String> {
    let reg = sync_host_workspace_registry()?;
    let mut nodes: Vec<WikiNode> = Vec::new();
    let mut links: Vec<WikiLink> = Vec::new();
    let mut note_count = 0usize;

    // Project → focus group (for hub coloring + membership edges).
    let projects = crate::projects_ops::projects_list().unwrap_or_default();
    let project_fg: HashMap<String, Option<String>> = projects
        .iter()
        .map(|p| (p.id.clone(), p.focus_group_id.clone()))
        .collect();

    let settings = crate::app_settings::load();
    let focus_groups_on = settings.focus_groups_enabled;
    let all_groups = if focus_groups_on {
        crate::db_ops::focus_groups_list().unwrap_or_default()
    } else {
        Vec::new()
    };
    let group_by_id: HashMap<String, &crate::db::schema::FocusGroup> =
        all_groups.iter().map(|g| (g.id.clone(), g)).collect();

    for w in &reg.workspaces {
        let ws_path = PathBuf::from(&w.path);
        let sub = match build_index(&ws_path) {
            Ok(i) => i,
            Err(_) => continue,
        };
        note_count += sub.note_count;

        let (fg_id, fg_name, fg_color) = if focus_groups_on {
            match project_fg.get(&w.id).and_then(|o| o.as_ref()) {
                Some(gid) => match group_by_id.get(gid.as_str()) {
                    Some(g) => (
                        Some(g.id.clone()),
                        Some(g.name.clone()),
                        g.color.clone(),
                    ),
                    None => (Some(gid.clone()), None, None),
                },
                None => (None, None, None),
            }
        } else {
            (None, None, None)
        };

        // Optional hub node per workspace (connects brains without polluting
        // per-workspace Home pages).
        let hub_id = fleet_node_id(&w.id, "__workspace__");
        nodes.push(WikiNode {
            id: hub_id.clone(),
            title: w.name.clone(),
            aliases: vec![],
            tags: vec!["workspace".into()],
            path: String::new(),
            exists: true,
            workspace_id: Some(w.id.clone()),
            workspace_name: Some(w.name.clone()),
            workspace_path: Some(w.path.clone()),
            kind: Some(NODE_KIND_WORKSPACE_HUB.into()),
            focus_group_id: fg_id.clone(),
            focus_group_name: fg_name.clone(),
            focus_group_color: fg_color.clone(),
            project_id: None,
            project_name: None,
            project_color: None,
        });

        for n in sub.nodes {
            let fid = fleet_node_id(&w.id, &n.id);
            let is_home = n.exists
                && (n.id.eq_ignore_ascii_case("Home.md")
                    || n.title.eq_ignore_ascii_case("Home"));
            if is_home {
                links.push(WikiLink {
                    source: hub_id.clone(),
                    target: fid.clone(),
                    missing: false,
                    kind: Some(LINK_KIND_WORKSPACE_HUB.into()),
                });
            }
            nodes.push(WikiNode {
                id: fid,
                title: n.title,
                aliases: n.aliases,
                tags: n.tags,
                path: n.path,
                exists: n.exists,
                workspace_id: Some(w.id.clone()),
                workspace_name: Some(w.name.clone()),
                workspace_path: Some(w.path.clone()),
                kind: Some(NODE_KIND_NOTE.into()),
                // Homes inherit group color for map identity; other notes stay clean.
                focus_group_id: if is_home { fg_id.clone() } else { None },
                focus_group_name: if is_home { fg_name.clone() } else { None },
                focus_group_color: if is_home { fg_color.clone() } else { None },
                project_id: None,
                project_name: None,
                project_color: None,
            });
        }
        for l in sub.links {
            links.push(WikiLink {
                source: fleet_node_id(&w.id, &l.source),
                target: fleet_node_id(&w.id, &l.target),
                missing: l.missing,
                kind: Some(LINK_KIND_WIKILINK.into()),
            });
        }
    }

    let wiki_ws_ids: std::collections::HashSet<String> =
        reg.workspaces.iter().map(|w| w.id.clone()).collect();

    // Focus-group hubs → workspace hubs (organizational edges, not wikilinks).
    let mut groups_out: Vec<WikiFocusGroup> = Vec::new();
    if focus_groups_on && !all_groups.is_empty() {
        for g in &all_groups {
            let member_ws: Vec<String> = reg
                .workspaces
                .iter()
                .filter(|w| {
                    project_fg
                        .get(&w.id)
                        .and_then(|o| o.as_deref())
                        == Some(g.id.as_str())
                        && wiki_ws_ids.contains(&w.id)
                })
                .map(|w| w.id.clone())
                .collect();

            groups_out.push(WikiFocusGroup {
                id: g.id.clone(),
                name: g.name.clone(),
                color: g.color.clone(),
                workspace_ids: member_ws.clone(),
            });

            // Still emit the group node even with zero wiki members so the
            // K2 filter can list every defined group.
            let gid = focus_group_node_id(&g.id);
            nodes.push(WikiNode {
                id: gid.clone(),
                title: g.name.clone(),
                aliases: vec!["Focus Group".into()],
                tags: vec!["focus-group".into()],
                path: String::new(),
                exists: true,
                workspace_id: None,
                workspace_name: None,
                workspace_path: None,
                kind: Some(NODE_KIND_FOCUS_GROUP.into()),
                focus_group_id: Some(g.id.clone()),
                focus_group_name: Some(g.name.clone()),
                focus_group_color: g.color.clone(),
                project_id: None,
                project_name: None,
                project_color: None,
            });

            for ws_id in &member_ws {
                links.push(WikiLink {
                    source: gid.clone(),
                    target: fleet_node_id(ws_id, "__workspace__"),
                    missing: false,
                    kind: Some(LINK_KIND_FOCUS_GROUP.into()),
                });
            }
        }
    }

    // Project hubs → workspace hubs (V1 projects; not wikilinks).
    let mut projects_out: Vec<WikiProject> = Vec::new();
    let all_projects = crate::project_groups::list_groups().unwrap_or_default();
    for p in &all_projects {
        let members = crate::project_groups::list_members(&p.id).unwrap_or_default();
        let member_ws: Vec<String> = members
            .into_iter()
            .map(|m| m.workspace_id)
            .filter(|id| wiki_ws_ids.contains(id))
            .collect();

        projects_out.push(WikiProject {
            id: p.id.clone(),
            name: p.name.clone(),
            color: p.color.clone(),
            workspace_ids: member_ws.clone(),
        });

        let pid = project_node_id(&p.id);
        nodes.push(WikiNode {
            id: pid.clone(),
            title: p.name.clone(),
            aliases: vec!["Project".into()],
            tags: vec!["project".into()],
            path: String::new(),
            exists: true,
            workspace_id: None,
            workspace_name: None,
            workspace_path: None,
            kind: Some(NODE_KIND_PROJECT.into()),
            focus_group_id: None,
            focus_group_name: None,
            focus_group_color: None,
            project_id: Some(p.id.clone()),
            project_name: Some(p.name.clone()),
            project_color: p.color.clone(),
        });

        for ws_id in &member_ws {
            links.push(WikiLink {
                source: pid.clone(),
                target: fleet_node_id(ws_id, "__workspace__"),
                missing: false,
                kind: Some(LINK_KIND_PROJECT.into()),
            });
        }
    }

    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    links.sort_by(|a, b| {
        (
            &a.source,
            &a.target,
            a.kind.as_deref().unwrap_or(""),
        )
            .cmp(&(
                &b.source,
                &b.target,
                b.kind.as_deref().unwrap_or(""),
            ))
    });
    links.dedup_by(|a, b| {
        a.source == b.source && a.target == b.target && a.kind == b.kind
    });

    Ok(WikiIndex {
        workspace_path: crate::paths::k2_home().to_string_lossy().into_owned(),
        wiki_rel: HOST_WIKI_REL.into(),
        generated_at: chrono_now(),
        nodes,
        links,
        note_count,
        scope: "k2".into(),
        groups: groups_out,
        projects: projects_out,
    })
}

/// Read a note for a fleet node id (`wsId::Home.md`) or plain wiki-rel id
/// when `workspace` is provided.
pub fn read_note_fleet_or_local(
    workspace: Option<&Path>,
    id: &str,
) -> Result<WikiNoteBody, String> {
    if let Some(group_id) = id.strip_prefix(FOCUS_GROUP_NODE_PREFIX) {
        let name = crate::db_ops::focus_groups_list()
            .ok()
            .and_then(|gs| gs.into_iter().find(|g| g.id == group_id))
            .map(|g| g.name)
            .unwrap_or_else(|| "Focus Group".into());
        return Ok(WikiNoteBody {
            id: id.to_string(),
            title: name.clone(),
            aliases: vec!["Focus Group".into()],
            tags: vec!["focus-group".into()],
            body: format!(
                "# {name}\n\nFocus group hub on the K2 map. Membership edges go to workspace hubs, not article wikilinks.\n"
            ),
            path: String::new(),
            workspace_id: None,
            workspace_path: None,
        });
    }
    if let Some(project_id) = id.strip_prefix(PROJECT_NODE_PREFIX) {
        let name = crate::project_groups::get_group_by_id(project_id)
            .map(|g| g.name)
            .unwrap_or_else(|| "Project".into());
        return Ok(WikiNoteBody {
            id: id.to_string(),
            title: name.clone(),
            aliases: vec!["Project".into()],
            tags: vec!["project".into()],
            body: format!(
                "# {name}\n\nProject hub on the K2 map. Membership edges go to workspace hubs, not article wikilinks.\n"
            ),
            path: String::new(),
            workspace_id: None,
            workspace_path: None,
        });
    }
    if let Some((ws_id, note_id)) = parse_fleet_node_id(id) {
        if note_id == "__workspace__" {
            return Ok(WikiNoteBody {
                id: id.to_string(),
                title: "Workspace".into(),
                aliases: vec![],
                tags: vec!["workspace".into()],
                body: format!("# Workspace\n\nFleet hub node for workspace id `{ws_id}`.\n"),
                path: String::new(),
                workspace_id: Some(ws_id.to_string()),
                workspace_path: None,
            });
        }
        // Resolve path from registry / projects
        let reg = sync_host_workspace_registry()?;
        let entry = reg
            .workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .ok_or_else(|| format!("unknown workspace in fleet id: {ws_id}"))?;
        let mut body = read_note(Path::new(&entry.path), note_id)?;
        body.id = id.to_string();
        body.workspace_id = Some(entry.id.clone());
        body.workspace_path = Some(entry.path.clone());
        return Ok(body);
    }
    let ws = workspace.ok_or_else(|| "Missing project for non-fleet note id".to_string())?;
    read_note(ws, id)
}

/// Read one note body by wiki-relative id (e.g. `Home.md`). Confined.
pub fn read_note(workspace: &Path, id: &str) -> Result<WikiNoteBody, String> {
    if id.is_empty() || id.contains("..") || id.starts_with("missing:") {
        return Err("invalid note id".into());
    }
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("workspace path: {e}"))?;
    let wiki = root.join(WIKI_REL);
    if !wiki.is_dir() {
        return Err("wiki does not exist".into());
    }
    let wiki_canon = wiki.canonicalize().map_err(|e| format!("wiki path: {e}"))?;
    let candidate = wiki_canon.join(id);
    let abs = candidate
        .canonicalize()
        .map_err(|_| format!("note not found: {id}"))?;
    if !abs.starts_with(&wiki_canon) {
        return Err("note path escapes wiki root".into());
    }
    if !abs.is_file() {
        return Err(format!("note not found: {id}"));
    }
    let meta = fs::metadata(&abs).map_err(|e| e.to_string())?;
    if meta.len() > MAX_NOTE_BYTES {
        return Err("note too large".into());
    }
    let raw = fs::read_to_string(&abs).map_err(|e| e.to_string())?;
    let (fm, body) = split_frontmatter(&raw);
    let stem = abs
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.to_string());
    let title = fm
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(stem);
    Ok(WikiNoteBody {
        id: id.replace('\\', "/"),
        title,
        aliases: fm.aliases.unwrap_or_default(),
        tags: fm.tags.unwrap_or_default(),
        body,
        path: Path::new(WIKI_REL)
            .join(id)
            .to_string_lossy()
            .replace('\\', "/"),
        workspace_id: None,
        workspace_path: None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiNoteBody {
    pub id: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub body: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
}

/// Create seed `Home.md` + `_Index.md` if the wiki dir is empty/missing.
/// After any seed, syncs the host **wiki-index** at `~/.k2/wiki/` so the
/// new brain appears on the K2 fleet map. Workspace seeds never link to
/// the host index (one-way registration only).
pub fn seed_wiki(workspace: &Path) -> Result<Vec<String>, String> {
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("workspace path: {e}"))?;
    let wiki = root.join(WIKI_REL);
    fs::create_dir_all(&wiki).map_err(|e| format!("mkdir wiki: {e}"))?;
    let mut created = Vec::new();
    let home = wiki.join("Home.md");
    if !home.exists() {
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".into());
        fs::write(
            &home,
            format!(
                r#"---
title: Home
aliases: [Wiki Home, MOC]
tags: [moc, index]
---

# {name} — Knowledge Base

This is the living brain for this workspace. Notes use `[[wikilinks]]` to
connect ideas. Open **View Wiki** in K2 to explore the map.

## Start here
- [[_Index]] — terse pointer list

## Add notes
Create new `.md` files under `.k2/wiki/` and link them with `[[Note Title]]`.
"#
            ),
        )
        .map_err(|e| format!("write Home.md: {e}"))?;
        created.push("Home.md".into());
    }
    let index = wiki.join("_Index.md");
    if !index.exists() {
        fs::write(
            &index,
            r#"---
title: _Index
aliases: [Wiki Index]
tags: [index]
---

# _Index

Entry: [[Home]]

Add one line per note as the wiki grows.
"#,
        )
        .map_err(|e| format!("write _Index.md: {e}"))?;
        created.push("_Index.md".into());
    }
    // Deterministic host registration — best-effort (DB may be empty in unit tests).
    let _ = sync_host_workspace_registry();
    Ok(created)
}

// ── Internals ────────────────────────────────────────────────────────────

#[derive(Default)]
struct Frontmatter {
    title: Option<String>,
    aliases: Option<Vec<String>>,
    tags: Option<Vec<String>>,
}

fn split_frontmatter(raw: &str) -> (Frontmatter, String) {
    let trimmed = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    if !trimmed.starts_with("---") {
        return (Frontmatter::default(), raw.to_string());
    }
    let rest = &trimmed[3..];
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n")).unwrap_or(rest);
    let end = rest
        .find("\n---")
        .map(|i| i + 1) // point at ---
        .or_else(|| rest.find("\r\n---").map(|i| i + 2));
    let Some(end) = end else {
        return (Frontmatter::default(), raw.to_string());
    };
    let yaml = &rest[..end];
    let after = rest[end..]
        .strip_prefix("---")
        .unwrap_or(&rest[end..]);
    let body = after
        .strip_prefix('\n')
        .or_else(|| after.strip_prefix("\r\n"))
        .unwrap_or(after)
        .to_string();
    (parse_simple_frontmatter(yaml), body)
}

/// Minimal YAML subset for title/aliases/tags — no full YAML crate required.
fn parse_simple_frontmatter(yaml: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();
    let mut list_key: Option<&str> = None;
    let mut list_buf: Vec<String> = Vec::new();

    let flush = |key: Option<&str>, buf: &mut Vec<String>, fm: &mut Frontmatter| {
        if let Some(k) = key {
            if !buf.is_empty() {
                match k {
                    "aliases" => fm.aliases = Some(std::mem::take(buf)),
                    "tags" => fm.tags = Some(std::mem::take(buf)),
                    _ => buf.clear(),
                }
            }
        }
    };

    for line in yaml.lines() {
        let t = line.trim_end();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(item) = t.strip_prefix("- ") {
            if list_key.is_some() {
                let v = item.trim().trim_matches('"').trim_matches('\'').to_string();
                if !v.is_empty() {
                    list_buf.push(v);
                }
            }
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            flush(list_key, &mut list_buf, &mut fm);
            list_key = None;
            let key = k.trim();
            let val = v.trim();
            if val.is_empty() {
                if key == "aliases" || key == "tags" {
                    list_key = Some(key);
                    list_buf.clear();
                }
                continue;
            }
            // inline list: [a, b]
            if (key == "aliases" || key == "tags") && val.starts_with('[') {
                let inner = val.trim_start_matches('[').trim_end_matches(']');
                let items: Vec<String> = inner
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if key == "aliases" {
                    fm.aliases = Some(items);
                } else {
                    fm.tags = Some(items);
                }
                continue;
            }
            if key == "title" {
                fm.title = Some(val.trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    flush(list_key, &mut list_buf, &mut fm);
    fm
}

/// Extract wikilink targets: `[[Target]]`, `[[Target|alias]]`, `[[Target#h]]`.
pub fn extract_wikilink_targets(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // skip embeds ![[
            let embed = i > 0 && bytes[i - 1] == b'!';
            i += 2;
            let start = i;
            while i + 1 < bytes.len() && !(bytes[i] == b']' && bytes[i + 1] == b']') {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                break;
            }
            let inner = &body[start..i];
            i += 2;
            if embed {
                continue;
            }
            let target = inner
                .split('|')
                .next()
                .unwrap_or(inner)
                .split('#')
                .next()
                .unwrap_or(inner)
                .trim();
            if !target.is_empty() {
                out.push(target.to_string());
            }
        } else {
            i += 1;
        }
    }
    out
}

fn walk_md(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for ent in entries {
        let ent = ent.map_err(|e| e.to_string())?;
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk_md(&path, out)?;
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn normalize_key(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn insert_lookup(map: &mut HashMap<String, String>, key: &str, id: &str) {
    let k = normalize_key(key);
    if k.is_empty() {
        return;
    }
    // First wins (stable)
    map.entry(k).or_insert_with(|| id.to_string());
}

fn chrono_now() -> String {
    // Avoid chrono dep requirement in core if not already used heavily —
    // k2-core already has chrono.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn extract_wikilinks_basic() {
        let body = "See [[Home]] and [[Feature - Federations|fed]] and [[A#heading]].";
        let t = extract_wikilink_targets(body);
        assert_eq!(t, vec!["Home", "Feature - Federations", "A"]);
    }

    #[test]
    fn extract_skips_embeds() {
        let body = "pic ![[image.png]] and [[Note]]";
        let t = extract_wikilink_targets(body);
        assert_eq!(t, vec!["Note"]);
    }

    #[test]
    fn frontmatter_title_and_tags() {
        let raw = "---\ntitle: Hello\ntags: [a, b]\naliases: [Hi]\n---\n\nBody [[X]]\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm.title.as_deref(), Some("Hello"));
        assert_eq!(
            fm.tags.as_ref().map(|t| t.clone()),
            Some(vec!["a".into(), "b".into()])
        );
        assert!(body.contains("[[X]]"));
    }

    fn temp_ws(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-wiki-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn build_index_and_resolve() {
        let ws = temp_ws("idx");
        let wiki = ws.join(".k2/wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(
            wiki.join("Home.md"),
            "---\ntitle: Home\n---\n\nGo to [[Alpha]]\n",
        )
        .unwrap();
        fs::write(wiki.join("Alpha.md"), "# Alpha\n\nBack to [[Home]]\n").unwrap();

        let idx = build_index(&ws).unwrap();
        assert_eq!(idx.note_count, 2);
        assert!(idx.nodes.iter().any(|n| n.id == "Home.md" && n.exists));
        assert!(idx.links.iter().any(|l| l.source == "Home.md" && l.target == "Alpha.md"));
        assert!(idx.links.iter().any(|l| l.source == "Alpha.md" && l.target == "Home.md"));

        let note = read_note(&ws, "Home.md").unwrap();
        assert_eq!(note.title, "Home");
        assert!(note.body.contains("Alpha"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn seed_creates_home() {
        let ws = temp_ws("seed");
        let created = seed_wiki(&ws).unwrap();
        assert!(created.contains(&"Home.md".into()));
        assert!(ws.join(".k2/wiki/Home.md").is_file());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn missing_link_marked() {
        let ws = temp_ws("miss");
        let wiki = ws.join(".k2/wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("A.md"), "See [[Ghost]]\n").unwrap();
        let idx = build_index(&ws).unwrap();
        assert!(idx.links.iter().any(|l| l.missing && l.target.contains("Ghost")));
        assert!(idx.nodes.iter().any(|n| !n.exists && n.title == "Ghost"));
        let _ = fs::remove_dir_all(&ws);
    }
}
