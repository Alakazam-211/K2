// Home M4 — when a room tells its server "someone is watching this"
// (prd-home-multi-server-client MS39, GH#22).
//
// Server B's reaper ages a workspace out on its need clock (24 h default).
// A live terminal and connected viewers do NOT spare it; only a need does
// (`projects/activate`). So a room on B reports need to B, through the
// connection pool (`hostPool.keepRoomAlive`, B's own scope and login):
//
//   - when the room opens;
//   - when it gets focus;
//   - on the first input after an hour with none;
//   - once an hour while it is hot.
//
// The pool dedupes per (server, project) to once per 10 min, so a burst of
// focus changes costs one request. B decides; the room only reports need.
//
// TODO(M4-integrate): the pinned-room shell creates one per room with
// `hostPool.keepRoomAlive` and `room.scope.hostKey` / `room.activeProjectId()`,
// calls `opened()` after `room.tabs.room.open()`, `focused()` when
// `stores/window-room` focuses the room, `input()` from the room's key and
// pointer handlers, `setHot()` from `roomTiers.onTierChange`, and
// `dispose()` with `room.tabs.room.dispose()`. `createPinnedRoom`'s
// `activateProject` input becomes `(pid) => void hostPool.keepRoomAlive(scope.hostKey, pid)`.

import type { KeepAliveResult } from '@/lib/host-pool'

/** Input after this long with none re-sends the activate (MS39: 1 h). */
export const KEEP_ALIVE_IDLE_MS = 60 * 60_000
/** While hot, re-send this often (MS39: once an hour). */
export const KEEP_ALIVE_HOT_INTERVAL_MS = 60 * 60_000

export interface RoomKeepAliveDeps {
  hostKey: string
  projectId: string
  keepAlive(hostKey: string, projectId: string): Promise<KeepAliveResult>
  now(): number
  setInterval(fn: () => void, ms: number): unknown
  clearInterval(handle: unknown): void
}

export interface RoomKeepAlive {
  opened(): Promise<KeepAliveResult>
  focused(): Promise<KeepAliveResult>
  /** A key or pointer input in the room. Sends only after an idle hour. */
  input(): Promise<KeepAliveResult> | null
  /** Hot rooms re-send hourly; warm and cold rooms do not. */
  setHot(hot: boolean): void
  dispose(): void
}

export function createRoomKeepAlive(deps: RoomKeepAliveDeps): RoomKeepAlive {
  let lastActivity = deps.now()
  let interval: unknown = null
  let disposed = false

  const send = (): Promise<KeepAliveResult> => {
    lastActivity = deps.now()
    return deps.keepAlive(deps.hostKey, deps.projectId)
  }
  const stopInterval = (): void => {
    if (interval !== null) deps.clearInterval(interval)
    interval = null
  }

  return {
    opened() {
      if (disposed) throw new Error('room keep-alive used after dispose')
      return send()
    },
    focused() {
      if (disposed) throw new Error('room keep-alive used after dispose')
      return send()
    },
    input() {
      if (disposed) return null
      const idle = deps.now() - lastActivity
      if (idle < KEEP_ALIVE_IDLE_MS) {
        lastActivity = deps.now()
        return null
      }
      return send()
    },
    setHot(hot) {
      if (disposed) return
      if (!hot) {
        stopInterval()
        return
      }
      if (interval !== null) return
      interval = deps.setInterval(() => {
        void send()
      }, KEEP_ALIVE_HOT_INTERVAL_MS)
    },
    dispose() {
      disposed = true
      stopInterval()
    },
  }
}
