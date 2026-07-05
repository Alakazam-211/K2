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
