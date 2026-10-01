// Home M5 — may a terminal pane in this room call `sessions/v2/spawn`?
//
// A view-only room (the M4 preview, or a server too old for the layout
// revision check) only ever ATTACHES to a live session on its server. It
// sends `attach_only: true`, but only daemons that report the
// `spawn-attach-only` feature honour that flag: released daemons up to
// 0.41.6 ignore unknown body fields and would SPAWN a new session for a tab
// that is not running. So on such a server a view-only room first asks the
// server which sessions are live (`sessions/list-for-workspace`, a GET) and
// sends the spawn only for a tab whose session is listed there. Anything
// else shows "Not running on …" and nothing is sent.
//
// A usable room (and the window's own room) is unchanged: it spawns.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export type RoomSpawnPlan =
  /** Send the spawn. `attachOnly` ⇒ with `attach_only: true`. */
  | { kind: 'spawn'; attachOnly: boolean }
  /** The tab's session is not live on the room's server: send nothing. */
  | { kind: 'not-live' }

export interface RoomSpawnRoom {
  readonly readOnly: boolean
  readonly scope: ServerScope
  cwd(): string
}

/** Live sessions on the room's server for a workspace path (agent names). */
export type ListLiveAgents = (scope: ServerScope, path: string) => Promise<string[]>

export const listLiveAgents: ListLiveAgents = async (scope, path) => {
  const rows = await daemonCliGet<Array<{ agentName?: unknown }>>(scope, 'sessions/list-for-workspace', { path })
  if (!Array.isArray(rows)) throw new Error('sessions/list-for-workspace did not answer with a list')
  return rows.map((r) => r.agentName).filter((n): n is string => typeof n === 'string')
}

/**
 * Decide the pane's spawn. `cwd` is the pane's own cwd, used only when the
 * room has none. Throws when the live list cannot be read (the pane shows
 * the error; it never falls back to sending the spawn).
 */
export async function planRoomSpawn(
  room: RoomSpawnRoom,
  agentName: string,
  cwd: string,
  list: ListLiveAgents = listLiveAgents,
): Promise<RoomSpawnPlan> {
  if (!room.readOnly) return { kind: 'spawn', attachOnly: false }
  if (room.scope.serverSupports('spawn-attach-only')) return { kind: 'spawn', attachOnly: true }
  const path = room.cwd() || cwd
  const live = await list(room.scope, path)
  return live.includes(agentName) ? { kind: 'spawn', attachOnly: true } : { kind: 'not-live' }
}
