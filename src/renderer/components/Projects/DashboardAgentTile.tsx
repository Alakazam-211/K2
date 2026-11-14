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
import { Button, OverflowMenu, usePressSpring } from '@/components/ui'
import { PinnedSessionBody } from '@/components/SessionView/AgentSessionChrome'
import { ThreadOverlayColumn } from '@/components/SessionView/ThreadOverlayColumn'
import { ChatterOverlayPane } from '@/components/SessionView/ChatterOverlayPane'
import { InputsList } from '@/components/SessionView/ThreadDrawerPanel'
import { ThreadDrawerSelector, VIEW_LABEL, VIEW_TOOLTIP, ViewIcon } from '@/components/SessionView/ThreadDrawerSelector'
import { threadTextCapPx } from '@/components/SessionView/threadTextWidth'
import { DEFAULT_SPLIT_LEFT, DEFAULT_SPLIT_RIGHT, type SessionViewTab } from '@/components/SessionView/sessionViewTab'
import type { ServerScope } from '@/kessel/server-scope'
import {
  resolveAllMemberSessions,
  activateOnLiveSessionAttach,
  wakeCanonicalMemberSession,
  type MemberSessionPhase,
} from './wake-member-session'
import { resolveTileView, TILE_VIEWS, tileViewKey, type TileView } from './tile-view'
import { projectDefaultView, type ProjectDefaultView, type ProjectGroupMemberInfo } from './projects-api'
import { TileState, TileStateLink, TryAgain } from './TileState'
import { openOnAgentsPage } from '@/lib/page-selection'

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
  defaultView: ProjectDefaultView | undefined,
): [TileView, (next: TileView) => void] {
  const hostKey = useConnectHostStore((s) => activeHostKey(s.activeHost))
  const key = tileViewKey(hostKey, groupId, workspaceId)
  const projectDefault = projectDefaultView({ defaultView })
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

/** The tile's view picker: the drawer capsule in `switch` mode, a size
 *  smaller than elsewhere (`sm`) so it sits lightly in the bar's centre. */
export function TileViewPicker({ view, onView }: { view: TileView; onView: (next: TileView) => void }): JSX.Element {
  return <ThreadDrawerSelector mode="switch" size="sm" view={view} onView={onView} />
}

/** The picker folded to one button (a narrow tile, the owner's mockup): the
 *  current view's icon, opening the four views as a menu with the current
 *  one checked, so the name keeps its room. PaneChrome shows it in place of
 *  the capsule when the bar is short of room (tile-bar-fit.ts). */
export function TileViewMenu({ view, onView }: { view: TileView; onView: (next: TileView) => void }): JSX.Element {
  return (
    <OverflowMenu
      label={`View: ${VIEW_LABEL[view]}`}
      menuLabel="View"
      align="start"
      triggerProps={{ 'data-testid': 'tile-view-menu' }}
      trigger={
        <span className="flex items-center gap-1">
          <ViewIcon kind={view} />
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </span>
      }
      items={TILE_VIEWS.map((kind) => ({
        id: kind,
        label: VIEW_LABEL[kind],
        title: VIEW_TOOLTIP[kind],
        icon: <ViewIcon kind={kind} />,
        checked: kind === view,
        onSelect: () => {
          if (kind !== view) onView(kind)
        },
      }))}
    />
  )
}

// ── The bar's picture ────────────────────────────────────────────────────

/** The agent's picture for the tile's bar, 18px (the owner's mockup). */
export function TilePicture({ name, addr }: { name: string; addr: string }): JSX.Element {
  const room = useRoom()
  return (
    <span
      data-testid="tile-picture"
      data-k2-chamfer="control"
      aria-hidden="true"
      className="flex-shrink-0 block w-[18px] h-[18px] overflow-hidden rounded-[var(--radius-pill)] bg-[var(--color-bg-elevated)]"
    >
      {addr ? (
        <Suspense fallback={null}>
          <AgentAvatar addr={addr} label={name} host={room.scope.isPrimary ? null : room.scope.hostKey} size={18} />
        </Suspense>
      ) : null}
    </span>
  )
}

// ── The tile body ────────────────────────────────────────────────────────

export function DashboardAgentTile({
  workspaceId,
  member,
  dashboardId,
  initialPhase,
  view,
  addr,
  onAsleep,
}: {
  workspaceId: string
  /** The bar's "asleep" hint: told whenever the agent falls asleep or wakes. */
  onAsleep?: (asleep: boolean) => void
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
  // What a failure was about: reaching the session (the liveness read) or
  // waking it. The error phase carries only the server's reason.
  const [failedWake, setFailedWake] = useState(false)
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

  useEffect(() => {
    onAsleep?.(phase.kind === 'dormant')
  }, [phase.kind, onAsleep])

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
    setFailedWake(false)
    try {
      await wakeCanonicalMemberSession(workspaceId, projectPath, {
        activateProject,
        ensurePinnedChat: (project) => daemonCliPost(scope, 'workspace/ensure-pinned-chat', { project }),
      })
      setPhase({ kind: 'live' })
    } catch (e) {
      setFailedWake(true)
      setPhase({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }, [projectPath, workspaceId, scope])

  // Try again on a failed read: the failure stays up with its button busy
  // ("Trying again…") until the read answers.
  const [retrying, setRetrying] = useState(false)
  const retry = useCallback(async (): Promise<void> => {
    setRetrying(true)
    try {
      const next = await resolveMemberSession(scope, workspaceId)
      setFailedWake(false)
      setPhase(next)
    } finally {
      setRetrying(false)
    }
  }, [workspaceId, scope])

  if (!member || !projectPath) {
    return (
      <TileState title="This agent isn’t here any more">
        It left the project or was removed from this server.
      </TileState>
    )
  }

  const heartbeats =
    view === 'heartbeats' ? (
      <InputsList
        cwd={projectPath}
        active={pageLive}
        empty={
          <TileState
            title="No heartbeats yet"
            testId="tile-heartbeats-empty"
            action={
              <Button
                variant="quiet"
                size="md"
                onClick={() => useSettingsStore.getState().openSettings('projects', workspaceId, 'schedule')}
              >
                Set up heartbeats
              </Button>
            }
          >
            A heartbeat wakes {name} on a schedule. Each run shows here.
          </TileState>
        }
      />
    ) : null

  // Thread view, still checking: the live tile's tree with its spawn held
  // (TerminalPane `holdSpawn`). The Thread and its message box paint now,
  // and when the check finds the session live the same pane attaches, so
  // nothing remounts. Asleep or failed, the held pane goes without a spawn.
  const checkingThread = phase.kind === 'checking' && view === 'thread' && addr !== ''
  if (phase.kind === 'live' || checkingThread) {
    // One tree for every view, so TerminalPane never remounts on a switch.
    return (
      <TileHost view={view}>
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
              sessionId={phase.kind === 'live' ? phase.sessionId : undefined}
              holdSpawn={phase.kind !== 'live'}
              syncSizeOnShow
              pauseHiddenPaint
            />
          </PinnedSessionBody>
        </div>
      </TileHost>
    )
  }

  if (phase.kind === 'error') {
    return (
      <TileState
        error
        title={failedWake ? `Couldn’t wake ${name}` : `Couldn’t reach ${name}’s session`}
        detail={phase.message}
        testId="tile-error"
        action={<TryAgain busy={retrying} onClick={() => void (failedWake ? wake() : retry())} />}
      >
        Still failing?{' '}
        <TileStateLink onClick={() => openOnAgentsPage(workspaceId)}>Open {name} on the Agents page</TileStateLink>
      </TileState>
    )
  }

  // Not live (checking, waking, dormant): Heartbeats and Chatter read without
  // a session; the Thread shows its history at once (from the view cache).
  if (view === 'heartbeats') return <TileHost view={view}>{heartbeats}</TileHost>
  if (view === 'terminal' || !addr) {
    if (phase.kind === 'checking') return <TileState busy title={`Checking ${name}’s session…`} />
    if (phase.kind === 'waking') return <TileState busy title={`Waking ${name}…`}>It opens here once it’s up.</TileState>
    return (
      <TileState
        title={`${name} is asleep`}
        action={
          <Button variant="accent" size="md" onClick={() => void wake()}>
            Wake
          </Button>
        }
      >
        Its session isn&apos;t running right now.
      </TileState>
    )
  }
  if (view === 'chatter') {
    return (
      <TileHost view={view}>
        <div className="flex-1 min-h-0 min-w-0" data-testid="agent-session-chatter">
          <ChatterOverlayPane addr={addr} conversationId={null} active={pageLive} textCapPx={textCap} />
        </div>
        {/* Asleep: the Thread view's Wake bar, under the mailbox (Chatter
            reads without a session, so nothing else says it is asleep). */}
        {phase.kind === 'dormant' ? (
          <TileWakeBar name={name} onWake={() => void wake()} />
        ) : phase.kind === 'waking' ? (
          <TileWakeBar name={name} waking />
        ) : null}
      </TileHost>
    )
  }
  // Thread, asleep (still checking draws the held live tree above): a Wake
  // bar stands where the message box goes, and stays while it wakes. The Wake bar
  // sits under the Thread's column, as Chatter's does, not in its box: the
  // box keeps its own corners (Thread/chamfer.ts), so inside it Terminal's
  // cut (`data-k2-chamfer`) never reached the bar or its Wake.
  const wakeBar =
    phase.kind === 'dormant' ? (
      <TileWakeBar name={name} onWake={() => void wake()} />
    ) : phase.kind === 'waking' ? (
      <TileWakeBar name={name} waking />
    ) : null
  return (
    <TileHost view={view}>
      <ThreadOverlayColumn
        addr={addr}
        conversationId={null}
        active={pageLive}
        composeBar={null}
        agentName={name}
        textCapPx={textCap}
      />
      {wakeBar}
    </TileHost>
  )
}

/** The tile's body for a view. Its bell and search are in the bar
 *  (TileBarTools); the tile itself is their host. */
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
        className="flex items-center gap-3 min-h-9 pl-3.5 pr-1.5 py-1 border border-dashed border-[var(--color-border)] bg-[var(--color-bg-elevated)] rounded-[var(--radius-field)] type-control text-[var(--color-text-secondary)]"
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
