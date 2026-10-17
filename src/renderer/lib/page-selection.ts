// Home and Agents each keep their own selected agent (Rosson 2026-10-16:
// "The agent you had selected on the Agents page should be retained and not
// match whatever agent was selected on Home (even if it was an agent on
// this computer)").
//
// Both pages are the same room shell: the room on screen is the window's
// active workspace (`projects.activeProjectId`), and a Grok Bot chat
// (`grokbots.open`) covers it on either page. Everything that follows the
// room on screen (top bar title, pinned tab, drawers, Thread overlay, the
// workspace-switch focus) reads those. So the room stays the window's
// active workspace, and this module gives each page its own memory of it:
//
//   - Leaving a room page for the other one remembers what the page we left
//     had selected (its workspace and its Grok Bot chat) and puts back what
//     the page we arrive on had. Other pages in between (Projects, Tickets,
//     Wiki, Settings) change nothing: the room still belongs to the last
//     room page.
//   - Home only selects its own rows. A workspace on screen that is not a
//     row of the selected Home was picked for Agents (palette, rail,
//     notification) and stays for Agents.
//   - Home's remote rooms (`stores/home-rooms`) are Home's alone already:
//     Agents never shows them and they stay open across pages.
//   - A remembered workspace that is gone (deleted, another server) is
//     skipped; the page shows what it shows today.
//
// Per window and in memory only (each window has its own JS context), like
// `stores/page-view`: selection is per-client view state, never daemon
// state.

import { usePageViewStore, isRoomPage } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useGrokBotsStore, type GrokChatTarget } from '@/stores/grokbots'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { grokBotIdFromAddress } from '@/lib/grokbots'

export type RoomPage = 'agents' | 'home'

interface WorkspaceSelection {
  projectId: string
  workspaceId: string | null
}

interface PageSelection {
  workspace: WorkspaceSelection | null
  grok: GrokChatTarget | null
}

/** The room page the window's active workspace belongs to right now. */
let owner: RoomPage = 'agents'
let memory: Record<RoomPage, PageSelection> = freshMemory()

function freshMemory(): Record<RoomPage, PageSelection> {
  return { agents: { workspace: null, grok: null }, home: { workspace: null, grok: null } }
}

/** Is `projectId` a row of the selected Home on the connected server? */
export function isHomeRowWorkspace(projectId: string | null): boolean {
  if (!projectId) return false
  const home = selectedHome(useHomesStore.getState())
  const connectedKey = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  const projects = useProjectsStore.getState().projects
  return home.rows.some(
    (row) =>
      !grokBotIdFromAddress(row.address) &&
      parseHomeAddress(row.address)?.host === connectedKey &&
      findWorkspaceForRow(projects, row)?.id === projectId,
  )
}

function currentWorkspace(): WorkspaceSelection | null {
  const { activeProjectId, activeWorkspaceId } = useProjectsStore.getState()
  return activeProjectId ? { projectId: activeProjectId, workspaceId: activeWorkspaceId } : null
}

/** Select `sel` as the window's active workspace (no-op when it already
 *  is, or when the workspace is gone). */
function selectWorkspace(sel: WorkspaceSelection): void {
  const ps = useProjectsStore.getState()
  const project = ps.projects.find((p) => p.id === sel.projectId)
  if (!project) return
  const ws = sel.workspaceId ? project.workspaces.find((w) => w.id === sel.workspaceId) : undefined
  if (ps.activeProjectId === project.id && (!ws || ps.activeWorkspaceId === ws.id)) return
  if (ws) ps.setActiveWorkspace(project.id, ws.id)
  else ps.setActiveProject(project.id)
}

function setGrok(target: GrokChatTarget | null): void {
  const open = useGrokBotsStore.getState().open
  if (open === target) return
  if (open && target && open.hostKey === target.hostKey && open.agentId === target.agentId) return
  // Not `openChat`: that also hides Home's remote room, which is Home's.
  useGrokBotsStore.setState({ open: target })
}

function switchRoomPage(from: RoomPage, to: RoomPage): void {
  const workspace = currentWorkspace()
  memory[from].grok = useGrokBotsStore.getState().open
  let restoreWorkspace = true
  if (from === 'agents') {
    memory.agents.workspace = workspace
  } else if (isHomeRowWorkspace(workspace?.projectId ?? null)) {
    memory.home.workspace = workspace
  } else {
    // Not one of Home's rows: it was picked for Agents, which keeps it.
    restoreWorkspace = false
  }
  const target = memory[to]
  if (restoreWorkspace && target.workspace) {
    if (to === 'agents' || isHomeRowWorkspace(target.workspace.projectId)) selectWorkspace(target.workspace)
  }
  setGrok(target.grok)
}

/** Remember each room page's selection across page changes. Installed
 *  once per window (App). Returns the uninstall. */
export function installPageSelectionMemory(): () => void {
  const page = usePageViewStore.getState().page
  owner = page === 'home' ? 'home' : 'agents'
  return usePageViewStore.subscribe((state, prev) => {
    if (state.page === prev.page || !isRoomPage(state.page)) return
    const to: RoomPage = state.page === 'home' ? 'home' : 'agents'
    if (to === owner) return
    const from = owner
    owner = to
    switchRoomPage(from, to)
  })
}

/** Open a workspace on the Agents page (Zen's "Open in Agents" and any
 *  other action aimed at Agents): it becomes Agents' selection, whatever
 *  page the window is on. */
export function openOnAgentsPage(projectId: string, workspaceId?: string | null): void {
  memory.agents = { workspace: { projectId, workspaceId: workspaceId ?? null }, grok: null }
  usePageViewStore.getState().setPage('agents')
  // Already Agents' room (from Agents, or a page in between): select here.
  selectWorkspace({ projectId, workspaceId: workspaceId ?? null })
  setGrok(null)
}

/** The room page that owns the window's active workspace (tests). */
export function roomPageOwner(): RoomPage {
  return owner
}

export function __resetPageSelectionForTests(): void {
  owner = 'agents'
  memory = freshMemory()
}
