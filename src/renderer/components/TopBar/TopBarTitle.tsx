// The top bar's centre title (Agents and Home).
//
// Local agent (the window's own server): `agent / branch`, as before.
// 0.46.0 experiment (Rosson 2026-10-16): when Home shows an agent on ANOTHER
// server, the title leads with that server, drawn like the server switcher
// draws one: the server icon whose two LEDs are that server's status light,
// then its name (the picker's label, truncated with an ellipsis, the full
// name and status in the tooltip). Then `/ agent`, and `/ branch` once the
// room's workspace is known. The switcher itself keeps naming the window's
// server, and the right cluster no longer draws the room's server name.

import { useSyncExternalStore } from 'react'
import { K2_ICON_PX, ServerIcon } from '@/components/icons/K2Icons'
import { scopeForHost } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import type { HomeRoomPhase } from '@/stores/home-rooms'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { Room } from '@/stores/room'
import { statusColor } from './server-status'
import { roomServerState, usePoolHostStatus, type RoomServerState } from './top-bar-scope'

/** The other server a Home room is on, for the centre title. */
export interface TopBarRemoteServer {
  hostKey: string
  phase: HomeRoomPhase
}

/** Pure: the remote server's light (same colours as the switcher's) and the
 *  words for it. Offline / sign-in come from the pool; a room still being
 *  read is "connecting"; a list that could not be read is an error. */
export function remoteServerIndicator(
  phase: HomeRoomPhase,
  state: RoomServerState,
): { color: string; title: string } {
  if (state === 'offline') return { color: statusColor('offline'), title: 'Connection: offline' }
  if (state === 'signin') return { color: statusColor('offline'), title: 'Sign-in required' }
  if (phase === 'resolving') return { color: statusColor('connecting'), title: 'Connection: connecting' }
  if (phase === 'error' || phase === 'not-found') {
    return { color: statusColor('offline'), title: "Connection: can't read this agent there" }
  }
  return { color: statusColor('connected'), title: 'Connection: connected' }
}

/** Pure: the name of the workspace (branch) a room shows, from its own
 *  server's project list. Null until the room has a workspace and the list
 *  has it. */
export function roomWorkspaceName(roomId: string | null, projects: ProjectWithWorkspaces[]): string | null {
  if (!roomId) return null
  const bar = roomId.lastIndexOf('|')
  const ids = roomId.slice(bar + 1)
  const colon = ids.lastIndexOf(':')
  if (colon < 0) return null
  const projectId = ids.slice(0, colon)
  const workspaceId = ids.slice(colon + 1)
  const project = projects.find((p) => p.id === projectId)
  const ws = project?.workspaces?.find((w) => w.id === workspaceId)
  return ws?.name || ws?.branch || null
}

const noSubscribe = (): (() => void) => () => {}

/** The room's workspace (branch) name, live with its server's list. */
export function useRoomWorkspaceName(room: Room | null): string | null {
  return useSyncExternalStore(room ? room.projects.subscribe : noSubscribe, () =>
    room ? roomWorkspaceName(room.roomId(), room.projects.getState().projects) : null,
  )
}

function RemoteServerCrumb({ server }: { server: TopBarRemoteServer }): React.JSX.Element {
  // Re-render when the saved servers change (a rename shows at once).
  useConnectHostStore((s) => s.hosts)
  const label = scopeForHost(server.hostKey).label
  const status = usePoolHostStatus(server.hostKey)
  const ind = remoteServerIndicator(server.phase, roomServerState(status))
  return (
    <span
      data-testid="top-bar-title-server"
      className="flex min-w-0 items-center gap-1.5 text-[var(--color-text-secondary)]"
      title={`${label} — ${ind.title}`}
      aria-label={`${label} — ${ind.title}`}
    >
      {/* The switcher's size (`SERVER_ICON_PX` is A3's one top-bar size). */}
      <ServerIcon ledColor={ind.color} size={K2_ICON_PX} />
      <span className="max-w-[160px] truncate" data-testid="top-bar-title-server-name">
        {label}
      </span>
    </span>
  )
}

function Slash(): React.JSX.Element {
  return <span className="shrink-0 text-[var(--color-text-muted)]">/</span>
}

export default function TopBarTitle({
  agent,
  branch,
  remoteServer,
}: {
  /** The agent (project) name. */
  agent?: string
  /** The workspace (branch / worktree) name. */
  branch?: string
  /** Set when the agent is on another server than the window's. */
  remoteServer?: TopBarRemoteServer
}): React.JSX.Element {
  return (
    <div className="flex min-w-0 items-center gap-1.5 text-xs" data-testid="top-bar-title">
      {agent ? (
        <>
          {remoteServer && (
            <>
              <RemoteServerCrumb server={remoteServer} />
              <Slash />
            </>
          )}
          <span className="truncate text-[var(--color-text-secondary)]" data-testid="top-bar-title-agent">
            {agent}
          </span>
          {branch && (
            <>
              <Slash />
              <span
                className="truncate text-[var(--color-text-primary)] font-medium"
                data-testid="top-bar-title-branch"
              >
                {branch}
              </span>
            </>
          )}
        </>
      ) : (
        <span className="text-[var(--color-text-muted)]">No workspace selected</span>
      )}
    </div>
  )
}
