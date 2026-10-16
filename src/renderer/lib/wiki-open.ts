// Which wiki the top bar's Wiki tab opens (0.46.0, Rosson 2026-10-16).
//
// The wiki of the workspace you're looking at:
//   - Agents: the room's active workspace;
//   - Home: the selected agent's workspace when it is on this window's
//     server (a row of the selected Home, shown in the window's own room).
// A Home selection on another server, nothing selected, or any other page:
// the last wiki viewed in this window (when that workspace is still on this
// server), else the first workspace in the nav order (Pinned, then the list
// under the focus-group picker). No note about other servers (Rosson: "just
// show the wikis for the agents on that server").
//
// Per-window view state only: reads this window's stores, writes the page
// store. No daemon calls.

import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress, type HomeRowRef } from '@/lib/home-address'

/** The slice of a workspace row the choice needs. */
export interface WikiWorkspaceRef {
  id: string
  path: string
  name: string
  handle?: string | null
  pinned?: number | boolean | null
  focusGroupId?: string | null
  workspaces?: readonly { worktreePath: string | null }[]
}

export interface WikiDefaultInput {
  page: AppPage
  /** This window's server's workspaces, in the daemon's order. */
  projects: readonly WikiWorkspaceRef[]
  /** The window's room's active workspace. */
  activeProjectId: string | null
  /** Home: the selected Home's row addresses. */
  homeRows: readonly HomeRowRef[]
  /** Home: a room from another server is on screen. */
  remoteRoomShown: boolean
  /** The connected server's Home host key. */
  connectedKey: string
  /** The nav's focus-group pick (null = groups off / none). */
  activeFocusGroupId: string | null
  /** The last workspace wiki shown in this window. */
  lastWikiPath: string | null
}

/** Is `path` one of this server's workspaces (or one of its worktrees)? */
export function wikiPathOnServer(projects: readonly WikiWorkspaceRef[], path: string): boolean {
  return projects.some((p) => p.path === path || (p.workspaces ?? []).some((w) => w.worktreePath === path))
}

/** The nav's order: Pinned first, then the list under the focus-group
 *  picker (ungrouped rows included), then anything else. */
export function wikiNavOrder<P extends WikiWorkspaceRef>(
  projects: readonly P[],
  activeFocusGroupId: string | null,
): P[] {
  const pinned = projects.filter((p) => !!p.pinned)
  const listed = projects.filter(
    (p) => !p.pinned && (activeFocusGroupId === null || p.focusGroupId === activeFocusGroupId || p.focusGroupId == null),
  )
  const rest = projects.filter((p) => !pinned.includes(p) && !listed.includes(p))
  return [...pinned, ...listed, ...rest]
}

/** The workspace selected on Home when it is on this window's server. */
export function homeLocalSelection(input: WikiDefaultInput): WikiWorkspaceRef | null {
  if (input.remoteRoomShown || !input.activeProjectId) return null
  for (const row of input.homeRows) {
    const parsed = parseHomeAddress(row.address)
    if (!parsed || parsed.host !== input.connectedKey) continue
    const ws = findWorkspaceForRow([...input.projects], row)
    if (ws && ws.id === input.activeProjectId) return ws
  }
  return null
}

/** The wiki path the Wiki tab opens, or null when this server has no
 *  workspace at all (pure). */
export function defaultWikiPath(input: WikiDefaultInput): string | null {
  const { projects } = input
  let viewed: WikiWorkspaceRef | null = null
  if (input.page === 'agents') {
    viewed = input.activeProjectId ? projects.find((p) => p.id === input.activeProjectId) ?? null : null
  } else if (input.page === 'home') {
    viewed = homeLocalSelection(input)
  }
  if (viewed) return viewed.path
  if (input.lastWikiPath && wikiPathOnServer(projects, input.lastWikiPath)) return input.lastWikiPath
  return wikiNavOrder(projects, input.activeFocusGroupId)[0]?.path ?? null
}

/** The window's current inputs for `defaultWikiPath`. */
export function currentWikiDefaultInput(): WikiDefaultInput {
  const view = usePageViewStore.getState()
  const projects = useProjectsStore.getState()
  const groups = useFocusGroupsStore.getState()
  return {
    page: view.page === 'wiki' ? view.wikiReturnPage : view.page,
    projects: projects.projects,
    activeProjectId: projects.activeProjectId,
    homeRows: selectedHome(useHomesStore.getState()).rows,
    remoteRoomShown: useHomeRoomsStore.getState().shown !== null,
    connectedKey: activeHomeHostKey(useConnectHostStore.getState().activeHost),
    activeFocusGroupId: groups.focusGroupsEnabled ? groups.activeFocusGroupId : null,
    lastWikiPath: view.lastWikiPath,
  }
}

/** The Wiki tab: open the wiki of the workspace on screen (see above). */
export function openWikiForView(): void {
  usePageViewStore.getState().openWiki(defaultWikiPath(currentWikiDefaultInput()))
}
