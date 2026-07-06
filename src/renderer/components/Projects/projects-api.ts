// Projects V1 P4 — renderer-side client for the daemon's
// `/cli/project-group/*` routes (P2, project_group_routes.rs) + the pure
// helpers the Projects page renders from.
//
// Wire shapes mirror k2-core's `ProjectGroup`/`ProjectGroupDashboard`
// (camelCase serde — crates/k2-core/src/project_groups.rs) and the show
// route's enriched member rows. All calls ride the HOST-AWARE
// daemonCliGet/daemonCliPost layer (the feedback-api idiom), so remoting
// into a server renders THAT server's projects.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

/** One group row (`/cli/project-group/list` + the create/pin responses). */
export interface ProjectGroup {
  id: string
  name: string
  /** `projects.id` of the PoC; null only while the group is memberless. */
  pocWorkspaceId: string | null
  /** Canonical nav Pinned-section flag (resolved Q4). */
  pinned: boolean
  sortOrder: number
  createdAt: number
  updatedAt: number
  memberCount: number
}

/** A member row as `show` enriches it (workspace registry name/path +
 *  agent display name). name/path are null when the workspace has been
 *  unregistered since it was added. */
export interface ProjectGroupMemberInfo {
  workspaceId: string
  name: string | null
  path: string | null
  agentName: string | null
  createdAt: number
}

export interface ProjectGroupDashboard {
  id: string
  groupId: string
  name: string
  /** Versioned layout blob (§6.3) — P5 parses it; P4 renders placeholders. */
  layoutJson: string
  revision: number
  position: number
  createdAt: number
  updatedAt: number
}

/** The `show` wire shape: the group's fields flat + members + dashboards. */
export interface ProjectGroupShow extends ProjectGroup {
  members: ProjectGroupMemberInfo[]
  dashboards: ProjectGroupDashboard[]
}

export interface ProjectGroupMessage {
  id: string
  groupId: string
  author: string
  body: string
  createdAt: number
}

/** One page of the chat stream, oldest-first (`MessagePage` wire shape).
 *  `truncated` = more rows matched than the effective limit allowed. */
export interface ProjectGroupMessagesPage {
  messages: ProjectGroupMessage[]
  truncated: boolean
}

/** The `msg` POST response: the stored row + the §4.3 delivery outcome
 *  (`delivered` / `deliveryReason` / `deliveredSessionId`) — a delivery
 *  failure never fails the store, so this is always an Ok shape. */
export interface PostedProjectGroupMessage {
  id: string
  groupId: string
  author: string
  body: string
  createdAt: number
  delivered: boolean
  deliveryReason: string | null
  deliveredSessionId: string | null
}

/** GET /cli/project-group/list — all groups, pinned-first then
 *  sort_order then name (daemon-side ordering; the nav renders as-is). */
export async function fetchProjectGroups(): Promise<ProjectGroup[]> {
  const res = await daemonCliGet<{ ok: boolean; groups: ProjectGroup[] }>('project-group/list')
  return res.groups ?? []
}

/** GET /cli/project-group/show?group=<id|name> — one group with enriched
 *  members + its dashboards ('Main' only in V1). */
export async function fetchProjectGroupShow(group: string): Promise<ProjectGroupShow> {
  return daemonCliGet<ProjectGroupShow>('project-group/show', { group })
}

/** POST /cli/project-group/create — `{name}` → the new group (the daemon
 *  auto-creates its 'Main' dashboard). Throws on `name_taken`. */
export async function createProjectGroup(name: string): Promise<ProjectGroup> {
  return daemonCliPost<ProjectGroup>('project-group/create', { name })
}

/** POST /cli/project-group/pin — canonical nav Pinned-section flag. */
export async function pinProjectGroup(group: string, pinned: boolean): Promise<void> {
  await daemonCliPost('project-group/pin', { group, pinned })
}

/** POST /cli/project-group/rename — `{group, name}` → the renamed
 *  group. Throws on `name_taken` (P8 Settings surfaces it inline). */
export async function renameProjectGroup(group: string, name: string): Promise<ProjectGroup> {
  return daemonCliPost<ProjectGroup>('project-group/rename', { group, name })
}

/** POST /cli/project-group/delete — cascades the group's member/
 *  message/dashboard rows only, NEVER the workspaces themselves
 *  (locked default, §6.5 danger zone). */
export async function deleteProjectGroup(group: string): Promise<void> {
  await daemonCliPost('project-group/delete', { group })
}

/** POST /cli/project-group/add-member — `workspace` accepts a name,
 *  absolute path, or workspace UUID; the FIRST member of an empty
 *  group auto-becomes the PoC (daemon rule). */
export async function addProjectGroupMember(group: string, workspace: string): Promise<void> {
  await daemonCliPost('project-group/add-member', { group, workspace })
}

/** POST /cli/project-group/remove-member — removing the PoC throws the
 *  409 `poc_successor_required` backstop (the Settings UI disables the
 *  button first; §6.5). */
export async function removeProjectGroupMember(group: string, workspace: string): Promise<void> {
  await daemonCliPost('project-group/remove-member', { group, workspace })
}

/** POST /cli/project-group/set-poc — the reassignment dropdown's
 *  write; the target must already be a member (`not_a_member`). */
export async function setProjectGroupPoc(group: string, workspace: string): Promise<void> {
  await daemonCliPost('project-group/set-poc', { group, workspace })
}

/** POST /cli/project-group/dashboard/rename — P8's §6.5 Main-row
 *  rename (owner-or-admin gated daemon-side, like save-layout). Emits
 *  `project-group:groups-changed`, so open `show` views refetch. */
export async function renameProjectGroupDashboard(
  group: string,
  dashboardId: string,
  name: string,
): Promise<ProjectGroupDashboard> {
  return daemonCliPost<ProjectGroupDashboard>('project-group/dashboard/rename', {
    group,
    dashboardId,
    name,
  })
}

/** POST /cli/project-group/dashboard/create — `{group, name}` → the
 *  new dashboard (§6.7.6; owner-or-admin gated like save-layout).
 *  Throws on 409 `name_taken` (Settings surfaces it inline). Emits
 *  `project-group:groups-changed`, so open `show` views refetch. */
export async function createProjectGroupDashboard(
  group: string,
  name: string,
): Promise<ProjectGroupDashboard> {
  const res = await daemonCliPost<{ ok: boolean; dashboard: ProjectGroupDashboard }>(
    'project-group/dashboard/create',
    { group, name },
  )
  return res.dashboard
}

/** POST /cli/project-group/dashboard/delete — deleting a project's
 *  LAST dashboard is refused with 409 `last_dashboard` (§6.7.6; the
 *  Settings UI also disables the button when only one exists). Never
 *  touches sessions/workspaces. */
export async function deleteProjectGroupDashboard(
  group: string,
  dashboardId: string,
): Promise<void> {
  await daemonCliPost('project-group/dashboard/delete', { group, dashboardId })
}

/** POST /cli/project-group/dashboard/reorder — `{group, order}` writes
 *  the full id order → the reordered dashboard rows (new `position`s).
 *  Owner-or-admin gated; emits `project-group:groups-changed`. */
export async function reorderProjectGroupDashboards(
  group: string,
  order: string[],
): Promise<ProjectGroupDashboard[]> {
  const res = await daemonCliPost<{ ok: boolean; dashboards: ProjectGroupDashboard[] }>(
    'project-group/dashboard/reorder',
    { group, order },
  )
  return res.dashboards ?? []
}

/** One pinned-HTML doc from `GET /cli/project-group/html-docs` — an
 *  `isPinnedFile` file-viewer item out of a MEMBER workspace's
 *  `workspace_layouts` blob (§4.1/§6.5, member-only per resolved Q3). */
export interface ProjectGroupHtmlDoc {
  workspaceId: string
  workspaceName: string | null
  agentName: string | null
  filePath: string
  fileName: string
}

/** GET /cli/project-group/html-docs?group= — the §6.5 pinned-HTML
 *  browser's rows, deduped per (workspace, path), member order. */
export async function fetchProjectGroupHtmlDocs(group: string): Promise<ProjectGroupHtmlDoc[]> {
  const res = await daemonCliGet<{ ok: boolean; docs: ProjectGroupHtmlDoc[] }>(
    'project-group/html-docs',
    { group },
  )
  return res.docs ?? []
}

/** POST /cli/project-group/dashboard/save-layout — canonical
 *  last-write-wins save (§6.3a): the daemon bumps `revision` and emits
 *  `project-group:layout-changed {groupId, dashboardId, revision}`.
 *  Owner-or-admin gated daemon-side; the response is the saved
 *  dashboard row (its `revision` feeds the echo guard). */
export async function saveDashboardLayout(
  group: string,
  dashboardId: string,
  layoutJson: string,
): Promise<ProjectGroupDashboard> {
  return daemonCliPost<ProjectGroupDashboard>('project-group/dashboard/save-layout', {
    group,
    dashboardId,
    layoutJson,
  })
}

/** GET /cli/project-group/messages — the single chat stream,
 *  oldest-first. No `after` → the LATEST `limit` (default 20, max 500);
 *  `after` is strictly-greater unix seconds (the incremental path). The
 *  P6 drawer loads the recent page and "loads earlier" by re-reading
 *  the tail with a bigger limit (there is no `before` param). */
export async function fetchProjectGroupMessages(
  group: string,
  opts: { after?: number; limit?: number } = {},
): Promise<ProjectGroupMessagesPage> {
  const res = await daemonCliGet<{
    ok: boolean
    messages: ProjectGroupMessage[]
    truncated: boolean
  }>('project-group/messages', { group, after: opts.after, limit: opts.limit })
  return { messages: res.messages ?? [], truncated: res.truncated ?? false }
}

/** POST /cli/project-group/msg — post to the project chat AS THE HUMAN
 *  OWNER (`author` omitted → the daemon defaults it to `owner`, the app
 *  drawer's attribution seam). The daemon stores, emits
 *  `project-group:message-created`, then best-effort injects into the
 *  PoC's canonical session; the outcome rides the response (§4.3/§6.4). */
export async function postProjectGroupMessage(
  group: string,
  body: string,
): Promise<PostedProjectGroupMessage> {
  return daemonCliPost<PostedProjectGroupMessage>('project-group/msg', { group, body })
}

/** §4.4 badge reconciliation: a group is UNREAD when it has ≥1 chat
 *  message newer than the per-client last-seen cursor. One
 *  `messages?after=&limit=1` probe per group (the feedback waiting-count
 *  fan-out idiom); a failed probe counts as read — the badge is advisory
 *  and must never block or error the nav. */
export async function fetchUnreadGroupIds(
  groups: Array<{ id: string }>,
  lastSeenFor: (groupId: string) => number,
): Promise<string[]> {
  const flags = await Promise.all(
    groups.map(async (g) => {
      try {
        const res = await daemonCliGet<{ ok: boolean; messages: ProjectGroupMessage[] }>(
          'project-group/messages',
          { group: g.id, after: lastSeenFor(g.id), limit: 1 },
        )
        return (res.messages?.length ?? 0) > 0 ? g.id : null
      } catch {
        return null
      }
    }),
  )
  return flags.filter((id): id is string => id !== null)
}

// ── Pure helpers (unit-tested in projects-api.test.ts) ────────────────────

/** The daemon's project-group error contract is
 *  `{"ok":false,"error":{"code","hint"}}`; daemon-cli surfaces the RAW
 *  body as the thrown Error's message when `error` isn't a plain string.
 *  Recover the stable code + hint from it (best-effort — a non-JSON
 *  message yields neither). */
export function daemonErrorInfo(err: unknown): { code?: string; hint?: string } {
  const msg = err instanceof Error ? err.message : String(err)
  try {
    const parsed = JSON.parse(msg)
    const e = parsed?.error
    if (e && typeof e === 'object') {
      return {
        code: typeof e.code === 'string' ? e.code : undefined,
        hint: typeof e.hint === 'string' ? e.hint : undefined,
      }
    }
  } catch {
    /* not the JSON error shape — fall through */
  }
  return {}
}

/** User-facing message for a failed create — surfaces `name_taken`
 *  specifically (§6.1: the inline new-project input shows it). */
export function createErrorMessage(err: unknown): string {
  const { code, hint } = daemonErrorInfo(err)
  if (code === 'name_taken') return hint ?? 'A project with that name already exists.'
  if (hint) return hint
  return err instanceof Error ? err.message : String(err)
}

/** Nav partition: canonical Pinned section on top, unpinned below
 *  (Sidebar.tsx `pinnedProjects` idiom). The daemon already orders
 *  pinned-first / sort_order / name; this just splits the sections. */
export function partitionPinned<T extends { pinned: boolean }>(
  groups: T[],
): { pinned: T[]; unpinned: T[] } {
  return {
    pinned: groups.filter((g) => g.pinned),
    unpinned: groups.filter((g) => !g.pinned),
  }
}
