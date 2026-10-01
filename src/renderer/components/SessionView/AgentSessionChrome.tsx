import { useCallback, useEffect, useState, type JSX, type ReactNode } from 'react'
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
import { chatHarnessName, harnessFieldsKnown } from './chatHarness'
import { DEFAULT_SPLIT_LEFT, DEFAULT_SPLIT_RIGHT } from './sessionViewTab'
import { SessionViewChromeContext } from './sessionViewChrome'
import { useSessionViewTab } from './useSessionViewTab'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'
import { primaryScope } from '@/kessel/server-scope'

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
  const sessionKey = conversationId || addr || agentName
  const {
    viewTab,
    setViewTab,
    splitLeft,
    splitRight,
    setSplitLeft,
    setSplitRight,
  } = useSessionViewTab(sessionKey)
  const chatProvider = chatHarnessName({ command, commandHint, provider })
  const harnessKnown = harnessFieldsKnown({ command, commandHint, provider })
  useEffect(() => {
    if (!harnessKnown || chatProvider) return
    if (viewTab === 'chat') setViewTab('terminal')
    if (splitLeft === 'chat') setSplitLeft(DEFAULT_SPLIT_LEFT)
    if (splitRight === 'chat') setSplitRight(DEFAULT_SPLIT_RIGHT)
  }, [
    viewTab,
    splitLeft,
    splitRight,
    harnessKnown,
    chatProvider,
    setViewTab,
    setSplitLeft,
    setSplitRight,
  ])
  const [refreshing, setRefreshing] = useState(false)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)

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
        await daemonCliPost(primaryScope(), 'sessions/v2/refresh', {
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
          const { dropTabAfterFailedSidecarRefresh } = await import('@/stores/tabs')
          dropTabAfterFailedSidecarRefresh(refreshPaneGroupId)
        }
      }
    } finally {
      setRefreshing(false)
    }
  }, [refreshing, onRefresh, agentName, cwd])

  return (
    <SessionViewChromeContext.Provider
      value={{
        viewTab,
        splitLeft,
        splitRight,
        overlayAddr: addr,
        conversationId,
        chatConversationId: conversationId,
        chatProvider,
        agentName,
      }}
    >
      <div className="h-full flex flex-col min-h-0" data-testid="sidecar-session-chrome">
        <SidecarSessionHeader
          title={title}
          viewTab={viewTab}
          splitLeft={splitLeft}
          splitRight={splitRight}
          onViewTabChange={setViewTab}
          onSplitLeft={setSplitLeft}
          onSplitRight={setSplitRight}
          chatEligible={Boolean(chatProvider)}
          onRefresh={() => void handleRefresh()}
          refreshing={refreshing}
        />
        {refreshError ? (
          <p
            role="alert"
            data-testid="sidecar-refresh-error"
            className="px-3 py-1 text-xs text-[var(--color-status-error,var(--color-text-muted))]"
          >
            {refreshError}
          </p>
        ) : null}
        <div className="relative flex-1 min-h-0 overflow-hidden flex flex-col" data-testid="agent-session-terminal">
          <Remount key={nonce}>{children}</Remount>
        </div>
      </div>
    </SessionViewChromeContext.Provider>
  )
}

function Remount({ children }: { children: ReactNode }): JSX.Element {
  return <>{children}</>
}

export function SidecarSessionHeader({
  title,
  viewTab,
  splitLeft,
  splitRight,
  onViewTabChange,
  onSplitLeft,
  onSplitRight,
  chatEligible = false,
  onRefresh,
  refreshing,
}: {
  title: string
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  onViewTabChange: (tab: SessionViewTab) => void
  onSplitLeft: (view: SplitPaneView) => void
  onSplitRight: (view: SplitPaneView) => void
  chatEligible?: boolean
  onRefresh: () => void
  refreshing: boolean
}): JSX.Element {
  return (
    <div
      className="border-b border-[var(--color-border)] flex-shrink-0 px-3 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-stretch"
      data-testid="sidecar-session-header"
    >
        <div className="flex min-w-0 items-stretch">
        <SessionViewMenu
          value={viewTab}
          splitLeft={splitLeft}
          splitRight={splitRight}
          chatEligible={chatEligible}
          onChange={onViewTabChange}
          onSplitLeft={onSplitLeft}
          onSplitRight={onSplitRight}
        />
        </div>
        <span
          className="pt-2 pb-[7px] text-xs font-semibold text-[var(--color-text-primary)] truncate min-w-0 flex items-center justify-center text-center"
          data-testid="sidecar-session-title"
        >
          {title}
        </span>
        <div className="flex min-w-0 items-center justify-end">
        <button
          type="button"
          onClick={onRefresh}
          disabled={refreshing}
          title="Restart this session — respawns this PTY, does not switch Chat."
          aria-label="Refresh session"
          className="self-center inline-flex items-center justify-center h-5 w-5 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
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
  chatConversationId,
  chatProvider,
  agentName,
  children,
}: {
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  addr: string
  conversationId: string | null
  chatConversationId?: string | null
  chatProvider?: string | null
  agentName?: string
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
        chatConversationId: chatConversationId ?? conversationId,
        chatProvider: chatProvider ?? null,
        agentName: agentName ?? '',
      }}
    >
      <div className="relative flex-1 min-h-0 overflow-hidden flex flex-col" data-testid="agent-session-terminal">
        {children}
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

/** Resolve overlay addr for a sidecar pane (handle clipboard). */
export function useSidecarOverlayAddr(
  projectPath: string,
  paneGroupId: string,
  attachAgentName?: string,
): { title: string; addr: string } {
  const [state, setState] = useState({ title: '', addr: '' })

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
        const rows = await daemonCliGet<DaemonHandleRow[]>(primaryScope(), 'sessions/list-for-workspace', {
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
        setState({ title: found, addr: found })
        return
      }
      schedule()
    }

    setState({ title: '', addr: '' })
    void lookup()
    return () => {
      cancelled = true
      if (timer !== undefined) clearTimeout(timer)
    }
  }, [projectPath, paneGroupId, attachAgentName])

  return state
}
