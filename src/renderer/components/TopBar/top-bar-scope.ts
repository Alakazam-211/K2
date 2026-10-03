// 0.43.2 item 3 — the top bar follows the focused room
// (prd-home-seamless-0432 Z14–Z20, vs-live Z34–Z36).
//
// When a remote Home room is focused, the Agents/Home top bar shows THAT
// room's server: the switcher label, the presence roster, the usage chip,
// Keep awake and the mode toggle. The timer and the Tickets badge stay on
// the window's server (Z15, Z18).
//
//   - Following is opt-in (Z36): only `TopBar.tsx` passes `followRoom`.
//     Settings, Projects, Wiki, Tickets, Focus and the gate chrome never
//     do, so their bars always show the window's server.
//   - A followed room's requests go through `room.scope` (its own server,
//     its own login). Nothing ever falls back to the window's server (MS5):
//     an offline room shows "offline", never the window's numbers (Z19).
//   - A room on the window's own server (a moment during promotion, Z7) is
//     the window's server: the bar shows the window's entries.

import { createContext, useContext, useMemo, useSyncExternalStore } from 'react'
import { useWindowRoomStore } from '@/stores/window-room'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { getPoolStatusSource, type PoolHostStatus } from '@/lib/pool-hooks'
import type { Room } from '@/stores/room'

/** The store key of the window's own server's entry (usage, Keep awake):
 *  the primary scope's id. It is reset on every top-switcher change (Z11). */
export const WINDOW_TOP_BAR_KEY = primaryScope().id

export interface TopBarTarget {
  /** The followed pinned room, or null for the window's server. */
  readonly room: Room | null
  /** Where every top-bar request for this target goes. */
  readonly scope: ServerScope
  /** Per-server store key: `primary`, or the room's Home host key. */
  readonly key: string
  /** The followed room's server label; null for the window's server (the
   *  window's own label stays where each element reads it today). */
  readonly label: string | null
  readonly isWindowServer: boolean
}

const WINDOW_TARGET: TopBarTarget = Object.freeze({
  room: null,
  scope: primaryScope(),
  key: WINDOW_TOP_BAR_KEY,
  label: null,
  isWindowServer: true,
})

/** The window's own server as a top-bar target. */
export function windowTopBarTarget(): TopBarTarget {
  return WINDOW_TARGET
}

/** Pure: what the top bar shows for this page and focused room. */
export function topBarTargetFor(followRoom: boolean, page: AppPage, focused: Room | null): TopBarTarget {
  if (!followRoom || page !== 'home' || !focused || focused.isPrimary) return WINDOW_TARGET
  if (focused.scope.isWindowHost()) return WINDOW_TARGET
  return {
    room: focused,
    scope: focused.scope,
    key: focused.scope.hostKey,
    label: focused.scope.label,
    isWindowServer: false,
  }
}

/** Set by `TopBarUtilities` for its elements (Z36). */
export const TopBarFollowRoomContext = createContext(false)

/**
 * The server this top-bar element shows. `followRoom` defaults to the
 * enclosing `TopBarUtilities`' prop; the switcher passes its own.
 */
export function useTopBarScope(followRoom?: boolean): TopBarTarget {
  const fromContext = useContext(TopBarFollowRoomContext)
  const follow = followRoom ?? fromContext
  const page = usePageViewStore((s) => s.page)
  const focused = useWindowRoomStore((s) => s.focused)
  return useMemo(() => topBarTargetFor(follow, page, focused), [follow, page, focused])
}

const noSubscribe = (): (() => void) => () => {}

/** The pool's view of `hostKey` (reach, login, role), live. Null for the
 *  window's server or before the pool knows it. */
export function usePoolHostStatus(hostKey: string | null): PoolHostStatus | null {
  const src = getPoolStatusSource()
  return useSyncExternalStore(src ? src.subscribe : noSubscribe, () =>
    src && hostKey ? (src.getState().entries[hostKey] ?? null) : null,
  )
}

/** What a followed room's server can give the top bar right now (Z19). */
export type RoomServerState = 'ok' | 'offline' | 'signin'

export function roomServerState(status: PoolHostStatus | null): RoomServerState {
  if (!status) return 'ok'
  if (status.reach === 'offline') return 'offline'
  if (status.checkedAt === null) return 'ok'
  if (status.auth === 'signin-required' || status.auth === 'rotate-required' || status.auth === 'kicked') {
    return 'signin'
  }
  return 'ok'
}

/** Q3 (Rosson's default, 2026-10-03): from a room, only an Admin or Owner
 *  on that server may change its Keep awake. A Member, or a role the pool
 *  has not read yet, sees it read-only. The window's own server is not
 *  gated here: the daemon's route floor decides there. */
export function roleMayChangeKeepAwake(role: string | null): boolean {
  return role === 'owner' || role === 'admin'
}

/** May the top bar send Keep awake changes for `target`? */
export function keepAwakeMayChange(target: TopBarTarget, role: string | null): boolean {
  if (!target.room) return true
  if (target.room.readOnly) return false
  return roleMayChangeKeepAwake(role)
}
