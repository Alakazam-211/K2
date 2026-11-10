// prd-projects-threads-v1 §3: an agent tile on a project dashboard.
//
// The tile shows one view at a time, picked in its bar (the Thread view's
// drawer capsule in `switch` mode, ThreadDrawerSelector): Terminal, Thread,
// Chatter or Heartbeats. No Split, no drawers. The view comes from the
// project's default (`defaultView`, daemon-owned) unless the person switched
// this tile (`tile-view.ts`, in memory only). The tile never reads the
// Agents page's view memory, so switching a tile leaves the Agents page alone.
//
// The terminal (`TerminalPane`) stays mounted in every view of a live tile:
// it owns the session attach and the Thread's message box, and a view change
// never closes or re-attaches the session. Thread and Chatter are drawn by
// TerminalPane itself from the chrome context (`PinnedSessionBody`).
// Heartbeats is the workspace's fires list (the Thread drawer's), full-tile,
// with the session held under it unseen.
//
// Live/dormant handling is the canonical session's (`wake-member-session.ts`):
// a dormant agent's Thread is still readable, with a Wake bar where the
// message box goes.

import { lazy, Suspense, useCallback, useEffect, useState, type JSX, type ReactNode } from 'react'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { resolvePinnedChatCopyableAddress } from '@/lib/chat-session-tab'
import { activateProject } from '@/stores/projects'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useSettingsStore } from '@/stores/settings'
import { usePageLive } from '@/contexts/TabVisibilityContext'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import { Button } from '@/components/ui'
import { PinnedSessionBody } from '@/components/SessionView/AgentSessionChrome'
import { ThreadOverlayColumn } from '@/components/SessionView/ThreadOverlayColumn'
import { ChatterOverlayPane } from '@/components/SessionView/ChatterOverlayPane'
import { InputsList } from '@/components/SessionView/ThreadDrawerPanel'
import { ThreadFloatBell } from '@/components/SessionView/ThreadFloatBell'
import { ThreadFloatSearch } from '@/components/SessionView/ThreadFloatSearch'
import {
  THREAD_FLOATS_GRID,
  ThreadFloatScrim,
  threadFloatShown,
  useThreadFloats,
  type ThreadFloatsPlace,
} from '@/components/SessionView/threadFloats'
import { threadTextCapPx } from '@/components/SessionView/threadTextWidth'
import { DEFAULT_SPLIT_LEFT, DEFAULT_SPLIT_RIGHT, type SessionViewTab } from '@/components/SessionView/sessionViewTab'
import type { ServerScope } from '@/kessel/server-scope'
import {
  resolveAllMemberSessions,
  activateOnLiveSessionAttach,
  wakeCanonicalMemberSession,
  type MemberSessionPhase,
} from './wake-member-session'
import {
  projectDefaultView,
  resolveTileView,
  tileViewKey,
  type TileView,
} from './tile-view'
import type { ProjectGroupMemberInfo } from './projects-api'

const AgentAvatar = lazy(() =>
  import('@/components/Thread/ThreadSenderAvatars').then((m) => ({ default: m.AgentAvatar })),
)

export type TermPhase = { kind: 'checking' } | MemberSessionPhase | { kind: 'waking' }

/** Stable placeholders while the parent's batch resolve runs. */
export const PHASE_CHECKING: TermPhase = { kind: 'checking' }
export const PHASE_DORMANT: TermPhase = { kind: 'dormant' }

/** One lookup-by-agent (the retry path after the batch). */
async function resolveMemberSession(scope: ServerScope, workspaceId: string): Promise<TermPhase> {
  const map = await resolveAllMemberSessions([workspaceId], async (agent) => {
    const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(scope,
      'sessions/lookup-by-agent',
      { agent },
    )
    return { sessionAlive: lookup.sessionAlive, sessionId: lookup.sessionId }
  })
  return map[workspaceId] ?? PHASE_DORMANT
}

// ── The tile's view ──────────────────────────────────────────────────────

/** This tile's view and its switch. A stale switch (made against another
 *  project default) is dropped, so a default change resets the tile. */
export function useTileView(
  groupId: string,
  workspaceId: string,
  rawDefault: unknown,
): [TileView, (next: TileView) => void] {
  const hostKey = useConnectHostStore((s) => activeHostKey(s.activeHost))
  const key = tileViewKey(hostKey, groupId, workspaceId)
  const projectDefault = projectDefaultView(rawDefault)
  const entry = useProjectGroupsStore((s) => s.tileViews[key])
  const { view, stale } = resolveTileView(entry, projectDefault)
  useEffect(() => {
    if (stale) useProjectGroupsStore.getState().dropTileView(key)
  }, [stale, key])
  const setView = useCallback(
    (next: TileView) => useProjectGroupsStore.getState().setTileView(key, next, projectDefault),
    [key, projectDefault],
  )
  return [view, setView]
}

/** The agent's Thread address: the room's project list has the workspace
 *  handle; a list from before handles looks it up (AgentChatPane's rule). */
function useTileThreadAddr(workspaceId: string, path: string | null): string {
  const room = useRoom()
  const known = useRoomProjects(
    (ps) => (ps.find((p) => p.id === workspaceId) ?? ps.find((p) => p.path === path))?.handle?.trim() ?? '',
  )
  const [addr, setAddr] = useState(known)
  useEffect(() => {
    if (known) {
      setAddr(known)
      return
    }
    if (!path) return
    let cancelled = false
    resolvePinnedChatCopyableAddress(room.scope, path, workspaceId)
      .then((a) => {
        if (!cancelled && a?.clipboard) setAddr(a.clipboard)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [known, path, workspaceId, room.scope])
  return addr
}

/** What the session chrome is told. Heartbeats holds the session under the
 *  list in Chatter view: the grid stays hidden the tested way (TerminalPane's
 *  own `display:none`), and no message box mounts unseen. */
export function tileSessionViewTab(view: TileView): SessionViewTab {
  return view === 'heartbeats' ? 'chatter' : view
}

// ── The bar's picture ────────────────────────────────────────────────────

/** The agent's picture for the tile's bar, 16px. */
export function TilePicture({ name, addr }: { name: string; addr: string }): JSX.Element {
  const room = useRoom()
  return (
    <span
      data-testid="tile-picture"
      data-k2-chamfer="control"
      aria-hidden="true"
      className="flex-shrink-0 block w-4 h-4 overflow-hidden rounded-[var(--radius-pill)] bg-[var(--color-bg-elevated)]"
    >
      {addr ? (
        <Suspense fallback={null}>
          <AgentAvatar addr={addr} label={name} host={room.scope.isPrimary ? null : room.scope.hostKey} size={16} />
        </Suspense>
      ) : null}
    </span>
  )
}

// ── Floats over a tile's Thread or Chatter ───────────────────────────────

/** Only search and the bell float over a tile's Thread (search alone over
 *  its Chatter): the bar already names the agent and holds the picker. */
function TileFloats({ place, workspaceId }: { place: ThreadFloatsPlace; workspaceId: string }): JSX.Element {
  const ref = useThreadFloats(true)
  return (
    <div ref={ref} className={THREAD_FLOATS_GRID} data-thread-floats="" data-testid="tile-floats">
      <ThreadFloatScrim />
      <div className="relative z-[1] flex min-w-0 items-center gap-1.5 thread-float-group" data-compact="">
        {threadFloatShown('bell', place) ? <ThreadFloatBell projectId={workspaceId} /> : null}
        {threadFloatShown('search', place) ? <ThreadFloatSearch /> : null}
      </div>
    </div>
  )
}

function floatsPlaceFor(view: TileView): ThreadFloatsPlace | null {
  if (view === 'thread') return 'tile'
  if (view === 'chatter') return 'tileChatter'
  return null
}

// ── The tile body ────────────────────────────────────────────────────────

export function DashboardAgentTile({
  workspaceId,
  member,
  dashboardId,
  initialPhase,
  view,
  addr,
}: {
  workspaceId: string
  /** null when the workspace left the project or was unregistered: the tile
   *  degrades to a placeholder, never breaks. */
  member: ProjectGroupMemberInfo | null
  dashboardId: string
  /** Parent-batched liveness. Still checking: the batch hasn't committed. */
  initialPhase: TermPhase
  view: TileView
  /** The agent's Thread address (`useAgentTileAddr`). */
  addr: string
}): JSX.Element {
  // The tile sits in the window's primary room (ProjectDashboard).
  const scope = useRoom().scope
  const projectPath = member?.path ?? null
  const name = member?.agentName ?? member?.name ?? workspaceId.slice(0, 8)
  const [phase, setPhase] = useState<TermPhase>(initialPhase)
  const pageLive = usePageLive()
  const textCap = useSettingsStore((s) => threadTextCapPx(s.threadTextWidth))

  // Track parent batch updates; a local wake in flight wins.
  useEffect(() => {
    setPhase((prev) => {
      if (prev.kind === 'waking') return prev
      if (
        prev.kind === initialPhase.kind &&
        (prev.kind !== 'live' || initialPhase.kind !== 'live' || prev.sessionId === initialPhase.sessionId)
      ) {
        return prev
      }
      return initialPhase
    })
  }, [initialPhase])

  // A live attach means the client is watching: Active.
  useEffect(() => {
    if (phase.kind !== 'live') return
    activateOnLiveSessionAttach(workspaceId, activateProject)
  }, [phase.kind, workspaceId])

  // Wake a dormant canonical session: activate, then ensure-pinned-chat
  // (without activate the reaper takes the chat PTY after ~15s).
  const wake = useCallback(async (): Promise<void> => {
    if (!projectPath) return
    setPhase({ kind: 'waking' })
    try {
      await wakeCanonicalMemberSession(workspaceId, projectPath, {
        activateProject,
        ensurePinnedChat: (project) => daemonCliPost(scope, 'workspace/ensure-pinned-chat', { project }),
      })
      setPhase({ kind: 'live' })
    } catch (e) {
      setPhase({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }, [projectPath, workspaceId, scope])

  const retry = useCallback(async (): Promise<void> => {
    setPhase({ kind: 'checking' })
    setPhase(await resolveMemberSession(scope, workspaceId))
  }, [workspaceId, scope])

  if (!member || !projectPath) {
    return (
      <PaneBody>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[36ch]">
          This workspace is no longer available — it left the project or was removed from the server.
        </p>
      </PaneBody>
    )
  }

  const place = floatsPlaceFor(view)
  const floats = place ? <TileFloats place={place} workspaceId={workspaceId} /> : null
  const heartbeats = view === 'heartbeats' ? <InputsList cwd={projectPath} active={pageLive} /> : null

  if (phase.kind === 'live') {
    // One tree for every view, so TerminalPane never remounts on a switch.
    return (
      <TileHost view={view}>
        {floats}
        {heartbeats}
        <div
          className="relative flex-1 min-h-0 min-w-0 flex flex-col"
          style={view === 'heartbeats' ? { display: 'none' } : undefined}
          data-testid="tile-session"
        >
          <PinnedSessionBody
            viewTab={tileSessionViewTab(view)}
            splitLeft={DEFAULT_SPLIT_LEFT}
            splitRight={DEFAULT_SPLIT_RIGHT}
            addr={addr}
            conversationId={null}
            agentName={workspaceId}
            displayName={name}
            terminalRevealed={false}
            terminalRevealKey={null}
            projectPath={projectPath}
            viewSwitch
          >
            <TerminalPane
              terminalId={`proj-dash:${dashboardId}:${workspaceId}`}
              cwd={projectPath}
              attachAgentName={workspaceId}
              sessionId={phase.sessionId}
              syncSizeOnShow
            />
          </PinnedSessionBody>
        </div>
      </TileHost>
    )
  }

  if (phase.kind === 'error') {
    return (
      <PaneBody>
        <p className="text-[11px] text-[var(--color-status-error-soft)] max-w-[36ch]">{phase.message}</p>
        <PaneActionButton onClick={() => void retry()}>Retry</PaneActionButton>
      </PaneBody>
    )
  }

  // Not live (checking, waking, dormant): Heartbeats and Chatter read without
  // a session; the Thread shows its history at once (from the view cache).
  if (view === 'heartbeats') return <TileHost view={view}>{heartbeats}</TileHost>
  if (view === 'terminal' || !addr) {
    if (phase.kind === 'checking') return <PaneBody>Checking session…</PaneBody>
    if (phase.kind === 'waking') return <PaneBody>Waking session…</PaneBody>
    return (
      <PaneBody>
        <p className="text-xs font-semibold text-[var(--color-text-primary)]">{name} is asleep</p>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[36ch]">
          Its session isn&apos;t running right now.
        </p>
        <PaneActionButton accent onClick={() => void wake()}>
          Wake
        </PaneActionButton>
      </PaneBody>
    )
  }
  if (view === 'chatter') {
    return (
      <TileHost view={view}>
        {floats}
        <div className="flex-1 min-h-0 min-w-0" data-testid="agent-session-chatter">
          <ChatterOverlayPane addr={addr} conversationId={null} active={pageLive} textCapPx={textCap} />
        </div>
      </TileHost>
    )
  }
  // Thread, not live: the box arrives with the session; asleep, a Wake bar
  // stands where it goes.
  const bar =
    phase.kind === 'dormant' ? (
      <TileWakeBar name={name} onWake={() => void wake()} />
    ) : phase.kind === 'waking' ? (
      <TileWakeBar name={name} waking />
    ) : null
  return (
    <TileHost view={view}>
      {floats}
      <ThreadOverlayColumn
        addr={addr}
        conversationId={null}
        active={pageLive}
        composeBar={bar}
        agentName={name}
        textCapPx={textCap}
      />
    </TileHost>
  )
}

/** The floats' host: the Thread under it pads by their height. */
function TileHost({ view, children }: { view: TileView; children: ReactNode }): JSX.Element {
  return (
    <div className="relative h-full min-h-0 min-w-0 flex flex-col overflow-hidden" data-testid="tile-body" data-tile-body={view}>
      {children}
    </div>
  )
}

/** Where an asleep agent's message box goes: "sales is asleep." and Wake. */
export function TileWakeBar({
  name,
  onWake,
  waking = false,
}: {
  name: string
  onWake?: () => void
  waking?: boolean
}): JSX.Element {
  return (
    <div className="px-3 pb-3 pt-1.5">
      <div
        data-testid="tile-wake-bar"
        data-tile-wake=""
        data-k2-chamfer="control"
        className="flex items-center gap-3 min-h-9 pl-3.5 pr-1.5 py-1 border border-dashed border-[var(--color-border)] bg-[var(--color-bg-elevated)] rounded-[var(--radius-field)] text-xs text-[var(--color-text-secondary)]"
      >
        <span className="min-w-0 flex-1 truncate" role="status">
          {waking ? `Waking ${name}…` : `${name} is asleep.`}
        </span>
        {waking ? null : (
          <Button variant="accent" onClick={onWake} data-testid="tile-wake">
            Wake
          </Button>
        )}
      </div>
    </div>
  )
}

/** The agent tile's Thread address, for the bar's picture and the body. */
export function useAgentTileAddr(workspaceId: string, member: ProjectGroupMemberInfo | null): string {
  return useTileThreadAddr(workspaceId, member?.path ?? null)
}

export function PaneBody({ children }: { children: ReactNode }): JSX.Element {
  return (
    <div className="h-full flex flex-col items-center justify-center gap-2 px-4 text-center text-[11px] text-[var(--color-text-muted)]">
      {children}
    </div>
  )
}

export function PaneActionButton({
  onClick,
  accent,
  children,
}: {
  onClick: () => void
  accent?: boolean
  children: ReactNode
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`px-3 py-1.5 text-[11px] font-medium transition-colors cursor-pointer ${
        accent
          ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25'
          : 'bg-[var(--color-wash-1)] text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)]'
      }`}
    >
      {children}
    </button>
  )
}
