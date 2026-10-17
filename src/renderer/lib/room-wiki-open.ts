// 0.46.0 (Rosson 2026-10-16) — View Wiki in a room on another server.
//
// The Wiki page reads the WINDOW's server (`wiki-api.ts` → `primaryScope()`),
// so a room on server B cannot open B's wiki in place (MS3: B's path means
// nothing on A). One click instead switches the window to B through the
// switcher's own path (`pickHost`: silent with a token, auto-login with a
// remembered password, else the normal full-screen sign-in), waits until B
// is connected and its project list has landed, then opens the Wiki page on
// that workspace (matched by its id on B, else its path).
//
// Sequence:
//   1. `requestHostSelect` so the window's room lands on the same workspace
//      (Home's own switch path; a shown room on B is promoted the same way).
//   2. `pickHost(target)`.
//   3. Wait: the window is on B, the gate said `connected`, and the projects
//      store (cleared on the switch) holds the workspace → `openWiki(path)`.
//
// A failed switch never opens the Wiki and leaves the page as it was; the
// switcher's own surfaces (sign-in, the gate) say why:
//   - the sign-in was cancelled (the window never left),
//   - B wants a sign-in again or is wedged once on it,
//   - the window went somewhere else, or the wait timed out.
// Home's remote rooms survive the switch (Z6/Z7): only rooms on B close.

import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { usePageViewStore } from '@/stores/page-view'
import { useToastStore } from '@/stores/toast'
import { activeHomeHostKey, homeAddress, workspaceHandle } from '@/lib/home-address'
import { PENDING_SELECT_TTL_MS, requestHostSelect } from '@/lib/home-pending-select'
import { switchTargetForHost } from '@/lib/home-switch'

export type RoomWikiOpenResult = 'opened' | 'switching' | 'unknown-server'

/** How long the open waits for the switch (the pending select's TTL: a
 *  sign-in can take a while). */
export const ROOM_WIKI_SWITCH_TTL_MS = PENDING_SELECT_TTL_MS

export interface RoomWikiWorkspace {
  id: string
  path: string
  name: string
  handle?: string | null
}

let cancelPending: (() => void) | null = null

/** Switch this window to `hostKey` and open the Wiki on `workspace` there. */
export function openWikiOnRoomServer(hostKey: string, workspace: RoomWikiWorkspace): RoomWikiOpenResult {
  cancelPending?.()
  const hostState = useConnectHostStore.getState()
  if (activeHomeHostKey(hostState.activeHost) === hostKey) {
    usePageViewStore.getState().openWiki(workspace.path)
    return 'opened'
  }
  const target = switchTargetForHost(hostKey, hostState.hosts)
  if (!target) {
    useToastStore.getState().addToast(`${hostKey} is not a saved server. Add it in Settings → Connections.`, 'warning')
    return 'unknown-server'
  }
  const targetId = target === 'local' ? 'local' : target.id

  const handle = workspaceHandle(workspace)
  if (handle) requestHostSelect(targetId, { address: homeAddress(handle, hostKey), workspaceId: workspace.id })

  let reached = false
  let sawSignIn = false
  let finished = false
  const finish = (): void => {
    if (finished) return
    finished = true
    unsubHost()
    unsubProjects()
    clearTimeout(timer)
    if (cancelPending === finish) cancelPending = null
  }
  const check = (): void => {
    if (finished) return
    const s = useConnectHostStore.getState()
    if (activeHomeHostKey(s.activeHost) !== hostKey) {
      // Left B after reaching it, or the sign-in for B was cancelled: stay put.
      if (reached) return finish()
      if (s.pendingSignIn && s.pendingSignIn.id === targetId) sawSignIn = true
      else if (sawSignIn) finish()
      return
    }
    reached = true
    if (s.recovery.kind === 'signin-required' || s.recovery.kind === 'wedged') return finish()
    if (s.connectionStatus !== 'connected') return
    const projects = useProjectsStore.getState().projects
    const match = projects.find((p) => p.id === workspace.id) ?? projects.find((p) => p.path === workspace.path)
    if (!match) return
    finish()
    usePageViewStore.getState().openWiki(match.path)
  }
  const unsubHost = useConnectHostStore.subscribe(check)
  const unsubProjects = useProjectsStore.subscribe(check)
  const timer = setTimeout(finish, ROOM_WIKI_SWITCH_TTL_MS)
  cancelPending = finish

  hostState.pickHost(target)
  check()
  return 'switching'
}

/** Tests: drop a wait in flight. */
export function __cancelRoomWikiOpenForTests(): void {
  cancelPending?.()
}
