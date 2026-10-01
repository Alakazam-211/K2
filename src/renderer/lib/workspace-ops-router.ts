// `workspace:*` Tauri events → the focused room (Home M3, MS18, answer Q4).
//
// The Workspace Assistant's tab tools emit `workspace:split-pane`,
// `workspace:open-terminal`, … to EVERY window (`workspace_ops.rs` uses
// `app.emit`). Before M3 the tabs module listened at import time and applied
// them to THE store. Now one window-level listener routes each op to the
// focused room's tabs store (`TabsRoomApi.applyWorkspaceOp`), so two mounted
// rooms never both act.
//
// Q4 (Rosson 2026-09-30): the assistant follows the server switcher. It acts
// only on a room on the window's connected server; in a room for any other
// server it refuses the action and says why.

import { primaryScope } from '@/kessel/server-scope'
import { focusedRoom } from '@/stores/window-room'
import { useToastStore } from '@/stores/toast'
import type { WorkspaceOp } from '@/stores/tabs'
import type { Room } from '@/stores/room'

const EVENTS: ReadonlyArray<[string, WorkspaceOp['kind']]> = [
  ['workspace:split-pane', 'split-pane'],
  ['workspace:close-pane', 'close-pane'],
  ['workspace:open-document', 'open-document'],
  ['workspace:open-terminal', 'open-terminal'],
  ['workspace:new-tab', 'new-tab'],
  ['workspace:close-tab', 'close-tab'],
  ['workspace:arrange', 'arrange'],
]

/** May the assistant act on `room`? Only a room on the window's server. */
export function assistantMayActOn(room: Room): boolean {
  return room.scope.hostKey === primaryScope().hostKey
}

/** Deliver one op: to the focused room only; nothing when no room is focused;
 *  refused (with a toast) in a room on another server. */
export function routeWorkspaceOp(op: WorkspaceOp): void {
  const room = focusedRoom()
  if (!room) return
  if (!assistantMayActOn(room)) {
    useToastStore
      .getState()
      .addToast(`The assistant can arrange rooms on ${primaryScope().label} only.`, 'warning')
    return
  }
  room.tabs.room.applyWorkspaceOp(op)
}

/** Listen once per window. Returns the teardown. `listen()` resolves after an
 *  await; a teardown that lands first unlistens each late registration. */
export function mountWorkspaceOpsRouter(): () => void {
  const unlisteners: Array<() => void> = []
  let torndown = false
  const track = (fn: () => void): void => {
    if (torndown) fn()
    else unlisteners.push(fn)
  }
  void import('@tauri-apps/api/event')
    .then(({ listen }) => {
      for (const [event, kind] of EVENTS) {
        listen<unknown>(event, (e) => {
          routeWorkspaceOp({ kind, payload: e.payload } as WorkspaceOp)
        })
          .then(track)
          .catch((err) => console.warn(`[workspace-ops] listen ${event} failed:`, err))
      }
    })
    .catch((err) => console.warn('[workspace-ops] Tauri event API unavailable:', err))
  return () => {
    torndown = true
    for (const fn of unlisteners.splice(0)) {
      try {
        void Promise.resolve(fn()).catch(() => {})
      } catch {
        /* listener registry already torn down */
      }
    }
  }
}
