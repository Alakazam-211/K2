# Mini-PRD — Workspace Resources (Files drawer + Projects Resources)

**Date:** 2026-08-23 · **Owner:** Rosson · **Status:** Locked for implement  
**Reviewed vs live code:** 2026-08-23 (patch after explore review)  
**Product:** daemon-first (`k2-core` + `k2-daemon`) + thin Files / Projects UI  
**Related:** `prd-projects-v1.md` §6.5 / ledger #9 (pinned HTML as project dashboard docs); `prd-html-dashboard.md` (cross-workspace HTML board — **not** this); `prd-file-viewer-preview-expansion-v1.md` (open-any-file in a pane — **xlsx out of scope there**)

---

## 1. Problem

Projects **Resources** today is a scrape of **pinned HTML tabs** on member workspaces (`isPinnedFile` inside `workspace_layouts`). Unpinning a tab removes it from the project. The Files drawer has no “this workspace’s resources” list — only a heuristic **AI Config** scan at the top of `FileTree`.

Rosson: resources should be files the user **explicitly** added from the tree, of **any type we can already open in a window**. Pinned HTML tabs stay as tab UX and **stop defining** project Resources.

---

## 2. Live truth (code, 2026-08-23)

| Surface | Today |
|---|---|
| Files drawer | `FileTree.tsx` in the Files panel tab. Top → bottom: **Environment** chips → **AI Config** chips → search → tree. `rootPath` = `activeWorkspace?.worktreePath ?? activeProject?.path` (`App.tsx`). |
| Environment / AI Config | Both `useState(false)` meaning **expanded** (not collapsed). AI Config: renderer `daemonCliGet('fs/read-dir', { path: rootPath, show_hidden: true })` then `AI_CONFIG_PATTERNS`. Empty → **omitted**. Collapse not persisted. Sparkle SVG, not Seti. |
| Tree context menu | New File/Folder, Copy/Cut/Paste, Rename, Duplicate, Compress, Extract (`.zip`), Download (remote/web), Trash, Reveal, Copy Path (`FileTree.tsx` `handleContextMenu` ~1447–1501). **No Add to Resources.** No viewer-mode gate in this menu. |
| Open from tree | `openFileAsTab` (`tabs.ts`). `getFileCategory`: md/html/image/pdf/docx/**csv\|tsv**/audio/video/zip/mermaid/diagram/binary/text. **`.xlsx` is not a table category** — falls through to **text**. Preview-expansion PRD explicitly out-of-scopes xlsx. |
| Pin HTML as tab | `FileViewerPane.handleTogglePin` calls `pinFileAsTab` **only when `category === 'html'`**; other types pin the **pane**. `pinFileAsTab` writes a leading tab with `isPinnedFile` + file-viewer `{ filePath }` (absolute host path), then `persistActiveWorkspace` → `POST /cli/workspace-layouts/save`. Does **not** check extension itself. |
| Layout blob | Serialized tab: top-level `isPinnedFile`, items `{ type: "file-viewer", filePath }` (not `data.filePath`). Save key `projectId:workspaceId` = `projects.id` + **worktree** `workspaces.id`. |
| Projects Resources | `ResourcesDrawer` **and** collapsed-rail `RailResource` in `ProjectNav.tsx`. Both `fetchProjectGroupHtmlDocs` → `GET /cli/project-group/html-docs?group=`. Hidden when 0 docs. Click/drag → `completeDashDrag({ type: 'htmlDoc' })` → dashboard **`HtmlDocPane` iframe only**. Context menu = **Pin/Unpin to Top only** (no Remove). |
| Settings “Pinned HTML pages” | Same GET (`ProjectSettings.tsx`). “Add to dashboard” → `appendHtmlDocPane` (`kind: 'htmlDoc'`). Empty copy still says members pin `.html` as tabs. |
| “Pin to Top” | Renderer `localStorage` `k2:project-nav:pinned-resources:${groupId}` (`member-pins.ts`) — sort only. |
| html-docs walker | `build_html_docs_json` / `pinned_html_paths` (`project_group_routes.rs` ~384–485). **Not** `.html`-filtered: `isPinnedFile` + `type == "file-viewer"` + `filePath`. Walks `tabs` and `extraGroups[].tabs`. SQL: `SELECT layout_json FROM workspace_layouts WHERE project_id = ?1` with **`project_group_members.workspace_id` = `projects.id`** (all worktree layout rows for that workspace). Wire: `{ ok, groupId, docs: [{ workspaceId, workspaceName, agentName, filePath, fileName }] }`. GET today; POST to it is **404** (not 405). |
| Persistence | **No** resources table. Latest SQL in `crates/k2-core/src/db/mod.rs` migrations array: **`0104_published_services`**. Next file is **`0105_…`**. (`0057` was skipped — do not invent it.) |
| Events | App-level pattern: `PublishServicesChanged` / `project_groups_changed` in `session_events.rs` + `session-events.ts` `onX`. Workspace-scoped events are filtered by path — **Projects page would miss those**. |

**IDs (do not mix):**

| Column / field | Meaning |
|---|---|
| PRD / members / `0104` `workspace_id` | **`projects.id`** of the workspace |
| `workspace_layouts.workspace_id` | **`workspaces.id`** (git worktree row) |
| `workspace_layouts.project_id` | **`projects.id`** (used by html-docs query) |

---

## 3. Lock

| # | Decision |
|---|---|
| R1 | A **resource** is a file the user added from the tree. **Not folders.** Any type the tree can already **open as a tab** (`openFileAsTab` / `getFileCategory`). That includes HTML, CSV/TSV, images, PDF, DOCX, md, zip, media, and “text/binary” fallbacks. **Do not claim Excel (`.xlsx`) table preview** — it is not a category today; adding an xlsx resource still opens the existing text/binary pane. |
| R2 | **Add:** Files tree → right-click a **file** (not directory) → **Add to Resources**. Idempotent: `INSERT OR IGNORE` / duplicate PK → one row, success. Resolve `workspace` as name \| path \| UUID (`resolve_workspace`), same as `/cli/workspace/set`. |
| R3 | **Files drawer chrome:** **Workspace Resources** section **above** AI Config. **Both default collapsed** (new; today both start expanded). Environment stays above both. **Empty Workspace Resources:** still **show** the collapsed header (unlike AI Config omit-when-empty) so Add has a home; empty Projects Resources drawer may keep hiding when 0 rows. |
| R4 | **Remove (workspace):** right-click a Workspace Resources row → **Remove**. |
| R5 | **Projects Resources:** union of Workspace Resources from **member workspaces** of that group (`list_members` + enrich `workspaceName` / `agentName` like today). Not layout scrape. Both **ResourcesDrawer and RailResource** retarget. |
| R6 | **Remove (project):** right-click a Projects Resources row → **Remove**. Deletes the **same** `workspace_resources` row (`workspaceId` + `filePath`). Viewer: FileTree has no extra gate; daemon `token_ok` like other `/cli` Files routes. **Do not** copy dashboard owner-or-admin unless a later pass says so. |
| R7 | **Pinned HTML tabs stay.** After ship, `pinFileAsTab` / `unpinFileTab` **only** mutate tabs + `workspace-layouts/save`. They **must not** INSERT/DELETE `workspace_resources`. |
| R8 | **Migration (once):** every `isPinnedFile` file-viewer path in that workspace’s layouts (`pinned_html_paths` — name is a lie, **not** HTML-only) is **inserted** if missing. SQL `0105` creates the table; **code migration** (`has_code_migration_applied` / `mark_code_migration_applied`, same as `workspace_layouts_dedup::run_once`) backfills at boot. Idempotent. **Do not** unpin tabs. **Do not** return an empty html-docs alias after migrate. |
| R9 | **Projects Resources live on the dashboard.** Click a row → open/focus a pane on the **mounted project dashboard** (same `insertEdge('right')` as member click; already-present → focus). Drag any resource onto the dashboard (not HTML-only). Layout kind stays `htmlDoc` `{workspaceId, filePath}` for blob compat. **HTML** panes keep the sandboxed iframe; **other types** render `FileViewerPane` in that pane. Files **drawer** click still `openFileAsTab` on that workspace. Settings “Add to dashboard” may still HTML-filter for iframe tiles, or add any resource — source list is Workspace Resources. |
| R10 | **Daemon-owned list.** SQLite. All clients + CLI + headless share one truth. Not `localStorage`, not renderer-only, not pinned-tab scrape. |
| R11 | **“Pin to Top”** on the project drawer stays **client sort** (`member-pins.ts`) in v1. |
| R12 | Duplicate add = one row. **Remove missing path → `404`** with `error.code` (not 200 no-op). Deleted-on-disk files **stay listed** until Remove; show a **missing** chip; open fails loud (existing FileViewer read error). Do not auto-drop. |

Daemon-first: if the list only exists while a webview is open, or diverges per client, it is in the wrong layer (GH#22 smell).

---

## 4. Implementation

### Schema

New file `crates/k2-core/drizzle_sql/0105_workspace_resources.sql` **and** an entry in the `migrations` array in `crates/k2-core/src/db/mod.rs` (SSOT is that array, not `schema.rs` alone).

```text
workspace_resources
  workspace_id  TEXT NOT NULL   -- projects.id of the workspace (NOT workspaces.id)
  file_path     TEXT NOT NULL   -- absolute daemon-host path (same as layouts / fs/read-file)
  added_at      INTEGER NOT NULL
  PRIMARY KEY (workspace_id, file_path)
```

**No SQL FK** (same comment as 0066 / `0104_published_services`: workspaces can be removed without FK). Do **not** copy `workspace_layouts` FKs (`workspace_id → workspaces.id`).

### Path confinement on add

`canonicalize` the path. Accept if it stays under **the Files tree root**: `workspaces.worktree_path` for that worktree **or** `projects.path` for the main checkout. **Reject** `starts_with(projects.path)`-only — that **drops worktree files** (`FileTree` root is `worktreePath ?? project.path`). Escape → **400**, no row.

### Routes

```text
GET  /cli/workspace/resources?workspace=<name|path|UUID>
POST /cli/workspace/resources/add     form/JSON: workspace, path
POST /cli/workspace/resources/remove  form/JSON: workspace, path
GET  /cli/project-group/resources?group=
GET  /cli/project-group/html-docs     -- COMPAT ALIAS: same payload as /resources
```

- GET list lives in `cli.rs` domain dispatch (`workspace_routes` / `project_group_routes`).
- POST mutators: add to **`dispatcher.rs` POST allowlist** (pattern: `/cli/workspace/set`, `/cli/project-group/*`, `publish_routes`). Twin GET on a mutator path → `CliResponse::method_not_allowed()` (**405**).
- `?workspace=` via `resolve_workspace` (`workspace_msg.rs`).
- Project GET: `list_members` → rows for those `projects.id`s. Wire **keep** `{ ok, groupId, docs: [{ workspaceId, workspaceName, agentName, filePath, fileName }] }` so Settings + old clients keep parsing. New UI may call `/resources` with the same shape.
- **html-docs alias returns the migrated resource list**, never a forced empty array.

### Events

New **app-level** `SessionEvent` (frozen-contract test in `session_events.rs`), e.g. `WorkspaceResourcesChanged { workspace_id }` (`projects.id`). Renderer: `session-events.ts` union + `onWorkspaceResourcesChanged` (pattern: `onPublishServicesChanged`, `onProjectGroupsChanged`).

**Must be app-level** (or Projects/Settings on the project page miss it). Do **not** `fetchProjects()`. Files section may optimistic-update; Projects/Settings refetch **this list** on the event.

### Migration

1. `0105` creates the table.
2. Boot: code migration scans every workspace’s `workspace_layouts` with **`pinned_html_paths`**, INSERT OR IGNORE into `workspace_resources`. Stamp `code_migrations`.
3. Never DELETE resources because a tab was unpinned.

### CLI

Optional `k2 resources list|add|rm` over the same HTTP. Not a ship blocker if curl-tests exist.

### Renderer

- `FileTree.tsx`: **Add to Resources** on files. **Workspace Resources** above AI Config; **both start collapsed**. Right-click row → **Remove**. Seti icons (`FileIcon`), not sparkle, not the generic Projects doc glyph.
- `ProjectNav.tsx`: `ResourcesDrawer` **and** `RailResource` fetch the new list. Right-click → **Remove**. **Click** → `requestFileDocPane` (dashboard `insertEdge('right')` / focus). **Drag any resource** onto the dashboard as `htmlDoc` `{workspaceId, filePath}`. HTML pane = iframe; other types = `FileViewerPane`.
- `ProjectSettings.tsx`: picker source = new list; **Add to dashboard** still HTML-only filter.
- Optimistic add/remove in Files only.

### Tests (loud)

- Add twice → one row.
- Path outside tree root (incl. `../` escape) → **400**, no row.
- Worktree file under `worktree_path` (not `projects.path`) → **200**, row exists.
- Remove missing → **404** + `error.code`.
- Project GET unions **members only**.
- Migration: layout `isPinnedFile` file-viewer → row; simulated unpin does **not** delete the row.
- html-docs GET after migrate returns **the same docs**, not `[]`.
- Mutating routes **405** on GET.
- `WorkspaceResourcesChanged` contract test (byte-stable kind).

Curl `/cli/workspace/resources` on a **headless** daemon before calling the feature done.

---

## 5. Out of scope

- Folders as resources.
- Auto-watching the tree to add files.
- Replacing or deleting pin-HTML-as-tab (`FileViewerPane` HTML pin affordance stays).
- New dashboard pane kinds for CSV/PDF/xlsx.
- `.xlsx` spreadsheet preview (existing file-viewer gap).
- `prd-html-dashboard.md` (global HTML board).
- Canonical shared sort / daemon pin-to-top (client `member-pins.ts` stays).
- Connect viewer ACL beyond existing `/cli` `token_ok`.

---

## 6. What to try (after ship)

1. Files tree → right-click a **CSV** → Add to Resources. Workspace Resources header is there (collapsed); AI Config still there (collapsed).
2. Right-click that row → Remove. Gone from Files **and** the project.
3. Pin an HTML tab: tab pins; **new** pins do **not** appear in Resources. Pins that existed **before** the upgrade **do** (migration).
4. Two workspaces in one project: each adds a file; Projects Resources shows both, labeled by member. Click a CSV → it opens as a pane **on the project dashboard** (FileViewer). Click HTML → iframe pane on the dashboard.
5. Drag any resource onto the project dashboard to place/split. Drag CSV must **not** jump to the Agents page.
6. Headless: `curl` list/add/remove; a second client sees the same list after `WorkspaceResourcesChanged`.
