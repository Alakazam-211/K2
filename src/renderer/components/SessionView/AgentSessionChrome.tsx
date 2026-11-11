import { useCallback, useEffect, useMemo, useRef, useState, type JSX, type ReactNode } from 'react'
import { subscribeThreadAddress, threadAddressChangeMatches } from '@/lib/thread-address-bus'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  beginSidecarRefresh,
  paneGroupIdFromTabAgent,
  settleSidecarRefresh,
} from '@/lib/sidecar-refresh-tab'
import {
  copyableAddressFromDaemonRow,
  type DaemonHandleRow,
} from '@/lib/chat-session-tab'
import { SessionViewMenu } from './SessionViewMenu'
import { ThreadViewIdentity } from './ThreadViewIdentity'
import { ThreadDrawerSelector } from './ThreadDrawerSelector'
import type { ThreadPanelKind } from './threadPanels'
import { ThreadFloatBell } from './ThreadFloatBell'
import { ThreadFloatSearch } from './ThreadFloatSearch'
import {
  THREAD_FLOATS_GRID,
  ThreadFloatScrim,
  floatProps,
  threadFloatShown,
  useThreadFloats,
  type ThreadFloatId,
  type ThreadFloatsPlace,
} from './threadFloats'
import { SPLIT_BAR_ATTR, SplitColumnFloatsContext, type SplitColumnFloatsSource } from './splitChrome'
import { chatHarnessName } from './chatHarness'
import { SessionViewChromeContext } from './sessionViewChrome'
import { useSessionViewTab } from './useSessionViewTab'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'
import { useRoom } from '@/components/Room/RoomContext'
import { SessionTokenPicker } from '@/components/Settings/shared/LlmLoginPicker'
import { roomProjectForCwd } from '@/stores/room'
import type { ServerScope } from '@/kessel/server-scope'
import ModeToggle, { ModeToggleSessionContext, type ModeToggleSession } from '@/components/Presence/ModeToggle'

interface AgentSessionChromeProps {
  /** Sidecar handle (`sales/reviewer`) or pinned workspace handle. */
  title: string
  addr: string
  conversationId: string | null
  /** v2_session_map key for this PTY — refresh resumes this key. */
  agentName: string
  /** Workspace cwd. Refresh reads the tab row for this project. */
  cwd?: string
  /** Spawn command. Restored tabs often only have `commandHint`. */
  command?: string
  commandHint?: string
  /** Pinned ensure provider, when the pane has no command prop. */
  provider?: string
  onRefresh?: () => void
  children: ReactNode
}

/** The remembered Thread|Terminal view's key (TR14): the conversation;
 *  before one is known (Codex before adoption, a fork) the pane, never the
 *  address, which a rename changes. */
export function sidecarViewKey(
  conversationId: string | null,
  agentName: string,
  addr: string,
): string {
  return conversationId || paneGroupIdFromTabAgent(agentName) || agentName || addr
}

/**
 * Sidecar chrome (C6/C7/C10): handle + Thread|Terminal + refresh.
 * No history dropdown. TerminalPane stays mounted (C4) via display:none.
 */
export function AgentSessionChrome({
  title,
  addr,
  conversationId,
  agentName,
  cwd,
  command,
  commandHint,
  provider,
  onRefresh,
  children,
}: AgentSessionChromeProps): JSX.Element {
  // Home M3 — refresh respawns on the room's server; a failed refresh drops
  // the tab from the room's strip.
  const room = useRoom()
  const sessionKey = sidecarViewKey(conversationId, agentName, addr)
  const {
    viewTab,
    setViewTab,
    splitLeft,
    splitRight,
    setSplitLeft,
    setSplitRight,
    terminalRevealed,
    setTerminalRevealed,
    terminalRevealKey,
    threadPanel,
    setThreadPanel,
  } = useSessionViewTab(sessionKey, room.scope)
  // Reported by the TerminalPane below (its phase).
  const [terminalLive, setTerminalLive] = useState(false)
  // The harness picks the Token control's provider below.
  const tokenProvider = chatHarnessName({ command, commandHint, provider })
  const [refreshing, setRefreshing] = useState(false)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  // The eye/pencil in any bar of this session reads its room.
  const modeSession = useMemo<ModeToggleSession>(() => ({ room }), [room])
  // The workspace's bell: Thread and Chatter views' floats, and split view's
  // Thread column (with the picture and name and search).
  const bell = useMemo(() => (cwd ? <ThreadFloatBell cwd={cwd} /> : null), [cwd])
  const splitFloats = useSplitColumnFloats(title, addr, bell)

  const handleRefresh = useCallback(async () => {
    if (refreshing) return
    setRefreshing(true)
    setRefreshError(null)
    // Only the `sessions/v2/refresh` branch. Pinned Chat passes onRefresh
    // and must not arm. Cleared when SessionRemoved is applied, not when
    // the POST returns — that response is not a fence for the broadcast.
    let refreshPaneGroupId: string | null = null
    try {
      if (onRefresh) {
        onRefresh()
      } else {
        refreshPaneGroupId = paneGroupIdFromTabAgent(agentName)
        if (refreshPaneGroupId) beginSidecarRefresh(refreshPaneGroupId)
        // Server kills and respawns the same provider session. The
        // nonce bump remounts so TerminalPane attaches to that PTY;
        // it is not a close-then-spawn.
        await daemonCliPost(room.scope, 'sessions/v2/refresh', {
          agent_name: agentName,
          ...(cwd ? { cwd } : {}),
        })
        if (refreshPaneGroupId) {
          settleSidecarRefresh(refreshPaneGroupId, { ok: true })
        }
        setNonce((n) => n + 1)
      }
    } catch (e) {
      const message = e instanceof Error && e.message ? e.message : 'Refresh failed'
      setRefreshError(message)
      if (refreshPaneGroupId) {
        const verdict = settleSidecarRefresh(refreshPaneGroupId, { ok: false, message })
        if (verdict === 'drop') {
          room.tabs.room.dropTabAfterFailedSidecarRefresh(refreshPaneGroupId)
        }
      }
    } finally {
      setRefreshing(false)
    }
  }, [room, refreshing, onRefresh, agentName, cwd])

  return (
    <SessionViewChromeContext.Provider
      value={{
        viewTab,
        splitLeft,
        splitRight,
        overlayAddr: addr,
        conversationId,
        agentName,
        displayName: title,
        terminalRevealed,
        terminalRevealKey,
        reportTerminalLive: setTerminalLive,
        threadPanel,
        setThreadPanel,
        projectPath: cwd,
      }}
    >
      <ModeToggleSessionContext.Provider value={modeSession}>
      <div className="relative h-full flex flex-col min-h-0" data-testid="sidecar-session-chrome">
        <SidecarSessionHeader
          title={title}
          addr={addr}
          terminalRevealed={terminalRevealed}
          onToggleTerminal={() => setTerminalRevealed(!terminalRevealed)}
          terminalAvailable={terminalLive}
          threadPanel={threadPanel}
          onThreadPanel={setThreadPanel}
          viewTab={viewTab}
          splitLeft={splitLeft}
          splitRight={splitRight}
          onViewTabChange={setViewTab}
          onSplitLeft={setSplitLeft}
          onSplitRight={setSplitRight}
          threadConversationId={conversationId}
          threadScope={room.scope}
          onRefresh={() => void handleRefresh()}
          refreshing={refreshing}
          bell={bell}
          tokenPicker={
            tokenProvider && !room.readOnly ? (
              <SessionTokenPicker
                scope={room.scope}
                provider={tokenProvider}
                sessionKey={agentName}
                conversationId={conversationId}
                resolveProjectId={() => (cwd ? roomProjectForCwd(room, cwd)?.id ?? null : null)}
                floating={sessionFloatsPlace(viewTab) !== null}
              />
            ) : null
          }
        />
        {refreshError ? (
          <p
            role="alert"
            data-testid="sidecar-refresh-error"
            className={`px-3 py-1 text-xs text-[var(--color-status-error,var(--color-text-muted))] ${
              viewTab === 'thread' || viewTab === 'chatter' ? 'thread-float-note' : ''
            }`}
          >
            {viewTab === 'thread' || viewTab === 'chatter' ? <span {...floatProps(true, 'note')}>{refreshError}</span> : refreshError}
          </p>
        ) : null}
        <div className="relative flex-1 min-h-0 overflow-hidden flex flex-col" data-testid="agent-session-terminal">
          <SplitColumnFloatsContext.Provider value={splitFloats}>
            <Remount key={nonce}>{children}</Remount>
          </SplitColumnFloatsContext.Provider>
        </div>
      </div>
      </ModeToggleSessionContext.Provider>
    </SessionViewChromeContext.Provider>
  )
}

function Remount({ children }: { children: ReactNode }): JSX.Element {
  return <>{children}</>
}

/** The controls Terminal view's bar draws. Thread view's objects that have
 *  no bar form (the terminal toggle, the bell, search) are not among them. */
export const BAR_CONTROLS: ReadonlySet<ThreadFloatId> = new Set<ThreadFloatId>(['view', 'token', 'history', 'refresh'])

/** The eye/pencil (`ModeToggle`) sits on Terminal view's bar only: where a
 *  terminal is shown. Not Thread view's floats (its slide-out terminal has
 *  no bar), not Chatter. Split view's bar places its own
 *  (`splitBarModeToggle`). */
export function barShowsModeToggle(viewTab: SessionViewTab, place: ThreadFloatsPlace | null): boolean {
  return place === null && viewTab === 'terminal'
}

/** Split view's one bar across both columns (Rosson, 2026-11-10): the view
 *  menu and side choosers, the picture and name (also on the Thread, so it
 *  shows with Chatter on both sides), and Token, the session and restart.
 *  Search and the bell belong to the Thread: its column floats them
 *  (SPLIT_VIEW_THREAD_FLOATS, SplitColumnFloats). The drawers' selector
 *  stays in Thread view: split view already picks its sides. */
export const SPLIT_BAR_CONTROLS: ReadonlySet<ThreadFloatId> = new Set<ThreadFloatId>([
  'view',
  'identity',
  'token',
  'history',
  'refresh',
])

/** Where a session header floats: Thread view or Chatter view; null is a
 *  bar (Terminal view, and split view's bar across both columns). */
export function sessionFloatsPlace(viewTab: SessionViewTab): ThreadFloatsPlace | null {
  if (viewTab === 'thread') return 'thread'
  if (viewTab === 'chatter') return 'chatter'
  return null
}

/** Whether a session header draws `id`: what its floats' place shows, else
 *  its bar's controls. */
export function sessionHeaderShows(id: ThreadFloatId, viewTab: SessionViewTab): boolean {
  const place = sessionFloatsPlace(viewTab)
  if (place) return threadFloatShown(id, place)
  return (viewTab === 'split' ? SPLIT_BAR_CONTROLS : BAR_CONTROLS).has(id)
}

/** The window's eye / pencil (viewer / claimer) is the terminal's: split
 *  view's bar draws it, before Token, when a side is a terminal. */
export function splitBarModeToggle(viewTab: SessionViewTab, left: SplitPaneView, right: SplitPaneView): boolean {
  return viewTab === 'split' && (left === 'terminal' || right === 'terminal')
}

/** What split view's floats column needs from the chrome (memoised). */
export function useSplitColumnFloats(name: string, addr: string, bell: ReactNode): SplitColumnFloatsSource {
  return useMemo(() => ({ name, addr, bell }), [name, addr, bell])
}

/** Split view's bar: Terminal view's bar (its height, rule and material),
 *  laid out on a grid whose sides never shrink below their controls, so the
 *  name in the middle shortens first. globals.css "Split view's bar" has the
 *  rest. */
export const SPLIT_BAR_GRID =
  'px-3 grid grid-cols-[minmax(max-content,1fr)_minmax(0,auto)_minmax(max-content,1fr)] gap-x-2 items-center'
/** Its three parts, each in its own column whatever is hidden. */
export const SPLIT_BAR_LEFT_CLASS = 'col-start-1 row-start-1 self-stretch flex items-stretch'
export const SPLIT_BAR_CENTRE_CLASS = 'col-start-2 row-start-1 min-w-0 flex justify-center'
export const SPLIT_BAR_RIGHT_CLASS = 'col-start-3 row-start-1 flex items-center justify-end gap-1.5'

export function SidecarSessionHeader({
  title,
  addr,
  terminalRevealed,
  onToggleTerminal,
  terminalAvailable,
  viewTab,
  splitLeft,
  splitRight,
  onViewTabChange,
  onSplitLeft,
  onSplitRight,
  onRefresh,
  refreshing,
  bell,
  tokenPicker,
  threadConversationId,
  threadScope,
  threadPanel,
  onThreadPanel,
}: {
  title: string
  /** The Thread's address: Thread view's header reads the picture by it. */
  addr: string
  terminalRevealed: boolean
  onToggleTerminal: () => void
  terminalAvailable: boolean
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  onViewTabChange: (tab: SessionViewTab) => void
  onSplitLeft: (view: SplitPaneView) => void
  onSplitRight: (view: SplitPaneView) => void
  onRefresh: () => void
  refreshing: boolean
  /** Thread view: the workspace's completion bell, as a floating object. */
  bell?: ReactNode
  /** The chat's Token control, left of refresh (agent chat tabs). */
  tokenPicker?: ReactNode
  /** thread-read-v1: this chat's Thread, for the view menu's unread + mute. */
  threadConversationId?: string | null
  threadScope?: ServerScope
  /** Thread view's Chatter / Heartbeats panel. No `onThreadPanel`: the drawer selector has the terminal only. */
  threadPanel?: ThreadPanelKind | null
  onThreadPanel?: (next: ThreadPanelKind | null) => void
}): JSX.Element {
  const floating = sessionFloatsPlace(viewTab) !== null
  const splitBar = viewTab === 'split'
  // A bar draws its own controls; floats draw what their place shows.
  const shows = (id: ThreadFloatId): boolean => sessionHeaderShows(id, viewTab)
  const floatsRef = useThreadFloats(floating)
  return (
    <div
      ref={floatsRef}
      className={
        floating
          ? THREAD_FLOATS_GRID
          : splitBar
            ? `h-8 border-b border-[var(--color-border)] flex-shrink-0 ${SPLIT_BAR_GRID}`
            : 'border-b border-[var(--color-border)] flex-shrink-0 px-3 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-stretch'
      }
      data-testid="sidecar-session-header"
      data-session-header=""
      data-thread-floats={floating ? '' : undefined}
      {...{ [SPLIT_BAR_ATTR]: splitBar ? '' : undefined }}
    >
        {floating ? <ThreadFloatScrim /> : null}
        <div
          className={
            floating
              ? 'relative z-[1] flex min-w-0 items-center gap-1.5 thread-float-group'
              : splitBar
                ? SPLIT_BAR_LEFT_CLASS
                : 'flex min-w-0 items-stretch'
          }
          data-split-bar-group={splitBar ? 'left' : undefined}
        >
        {shows('view') ? (
          <SessionViewMenu
            value={viewTab}
            splitLeft={splitLeft}
            splitRight={splitRight}
            onChange={onViewTabChange}
            onSplitLeft={onSplitLeft}
            onSplitRight={onSplitRight}
            threadConversationId={threadConversationId}
            threadScope={threadScope}
            floating={floating}
          />
        ) : null}
        {shows('drawers') ? (
          <ThreadDrawerSelector
            terminalRevealed={terminalRevealed}
            terminalAvailable={terminalAvailable}
            onToggleTerminal={onToggleTerminal}
            panel={threadPanel ?? null}
            onPanel={onThreadPanel}
          />
        ) : null}
        {floating && shows('bell') ? bell : null}
        {floating && shows('search') ? <ThreadFloatSearch /> : null}
        </div>
        {floating ? (
          <div className="relative z-[1] min-w-0">
            {shows('identity') ? <ThreadViewIdentity name={title} addr={addr} /> : null}
          </div>
        ) : splitBar ? (
          <div className={SPLIT_BAR_CENTRE_CLASS} data-split-bar-centre="">
            {shows('identity') ? <ThreadViewIdentity name={title} addr={addr} bar /> : null}
          </div>
        ) : (
          <span
            className="pt-2 pb-[7px] text-xs font-semibold text-[var(--color-text-primary)] truncate min-w-0 flex items-center justify-center text-center"
            data-testid="sidecar-session-title"
          >
            {title}
          </span>
        )}
        <div
          className={
            floating
              ? 'relative z-[1] flex min-w-0 items-center justify-end gap-1.5 thread-float-group'
              : splitBar
                ? SPLIT_BAR_RIGHT_CLASS
                : 'flex min-w-0 items-center justify-end gap-1'
          }
          data-split-bar-group={splitBar ? 'right' : undefined}
        >
        {barShowsModeToggle(viewTab, sessionFloatsPlace(viewTab)) ? <ModeToggle /> : null}
        {splitBarModeToggle(viewTab, splitLeft, splitRight) ? <ModeToggle /> : null}
        {shows('token') ? tokenPicker : null}
        {shows('refresh') ? (
          <button
            type="button"
            {...floatProps(floating, 'round')}
            onClick={onRefresh}
            disabled={refreshing}
            title="Restart this session — respawns this PTY, does not switch Chat."
            aria-label="Refresh session"
            className="self-center inline-flex items-center justify-center h-5 w-5 rounded-[var(--radius-field)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              className={refreshing ? 'animate-spin' : ''}
              aria-hidden="true"
            >
              <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
              <path d="M3 3v5h5" />
              <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
              <path d="M16 16h5v5" />
            </svg>
          </button>
        ) : null}
        </div>
    </div>
  )
}

export function PinnedSessionBody({
  viewTab,
  splitLeft,
  splitRight,
  addr,
  conversationId,
  agentName,
  displayName,
  terminalRevealed,
  terminalRevealKey,
  threadPanel,
  setThreadPanel,
  projectPath,
  splitFloats,
  viewSwitch,
  children,
}: {
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  addr: string
  conversationId: string | null
  agentName?: string
  /** AGENT.md display name — the Thread empty state names the agent. */
  displayName?: string
  /** Thread view's revealed terminal (`useSessionViewTab`). */
  terminalRevealed: boolean
  terminalRevealKey: string | null
  /** Thread view's Chatter / Heartbeats panel (`useSessionViewTab`). */
  threadPanel?: ThreadPanelKind | null
  setThreadPanel?: (next: ThreadPanelKind | null) => void
  /** The workspace's path, for the Heartbeats panel. */
  projectPath?: string
  /** Split view: what the Thread column's floats show (`useSplitColumnFloats`). */
  splitFloats?: SplitColumnFloatsSource
  /** An agent tile (Projects): views switch the whole tile (`viewSwitch` on
   *  the chrome). */
  viewSwitch?: boolean
  children: ReactNode
}): JSX.Element {
  return (
    <SessionViewChromeContext.Provider
      value={{
        viewTab,
        splitLeft,
        splitRight,
        overlayAddr: addr,
        conversationId,
        agentName: agentName ?? '',
        displayName,
        terminalRevealed,
        terminalRevealKey,
        threadPanel: setThreadPanel ? (threadPanel ?? null) : undefined,
        setThreadPanel,
        projectPath,
        viewSwitch,
      }}
    >
      <div className="relative flex-1 min-h-0 overflow-hidden flex flex-col" data-testid="agent-session-terminal">
        <SplitColumnFloatsContext.Provider value={splitFloats ?? null}>{children}</SplitColumnFloatsContext.Provider>
      </div>
    </SessionViewChromeContext.Provider>
  )
}

/** Re-ask until this cell's own address is in the session list. */
export const SIDECAR_OVERLAY_ADDR_RETRY_MS = 400

/**
 * Clipboard only when it is this sidecar (`sales/reviewer`, `sales/1`).
 * A slash-less handle is the workspace name — pinned Chat, not this cell —
 * so one early miss or a not-yet-stamped row must not stick.
 */
function sidecarOwnClipboard(row: DaemonHandleRow | undefined): string {
  if (!row) return ''
  const clipboard = copyableAddressFromDaemonRow(row)?.clipboard.trim() ?? ''
  if (!clipboard.includes('/')) return ''
  return clipboard
}

/** Resolve overlay addr for a sidecar pane (handle clipboard).
 *
 *  Thread survives a tab rename (S4): the address is looked up again, in
 *  place (no remount), when the daemon says this pane's address changed
 *  (`session_address_changed`, the overlay `address` frame) or a Thread
 *  call for the address it holds missed / answered `movedFrom`. */
export function useSidecarOverlayAddr(
  projectPath: string,
  paneGroupId: string,
  attachAgentName?: string,
  conversationId?: string | null,
): { title: string; addr: string } {
  const room = useRoom()
  const [state, setState] = useState({ title: '', addr: '' })
  const addrRef = useRef('')
  addrRef.current = state.addr
  const relookupRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    return subscribeThreadAddress((change) => {
      if (
        !threadAddressChangeMatches(change, {
          addr: addrRef.current,
          conversationId: conversationId ?? null,
          paneGroupId,
        })
      ) {
        return
      }
      // A known new address shows at once; the list confirms it.
      if (change.address && change.address.includes('/') && change.address !== addrRef.current) {
        setState({ title: change.address, addr: change.address })
      }
      relookupRef.current?.()
    })
  }, [paneGroupId, conversationId])

  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined

    const schedule = (): void => {
      if (cancelled) return
      timer = setTimeout(() => {
        void lookup()
      }, SIDECAR_OVERLAY_ADDR_RETRY_MS)
    }

    const lookup = async (): Promise<void> => {
      let found = ''
      try {
        const rows = await daemonCliGet<DaemonHandleRow[]>(room.scope, 'sessions/list-for-workspace', {
          path: projectPath,
        })
        if (cancelled) return
        const list = Array.isArray(rows) ? rows : []
        const name = attachAgentName || `tab-${paneGroupId}`
        const row =
          list.find((r) => r.agentName === name) ||
          list.find((r) => r.agentName === `tab-${paneGroupId}`)
        found = sidecarOwnClipboard(row)
      } catch {
        if (cancelled) return
      }
      if (cancelled) return
      if (found) {
        setState((prev) => (prev.addr === found ? prev : { title: found, addr: found }))
        return
      }
      schedule()
    }

    relookupRef.current = () => {
      if (cancelled) return
      if (timer !== undefined) clearTimeout(timer)
      timer = undefined
      void lookup()
    }
    setState({ title: '', addr: '' })
    void lookup()
    return () => {
      cancelled = true
      relookupRef.current = null
      if (timer !== undefined) clearTimeout(timer)
    }
  }, [room, projectPath, paneGroupId, attachAgentName])

  return state
}
