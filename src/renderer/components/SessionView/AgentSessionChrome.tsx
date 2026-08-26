import { useCallback, useEffect, useState, type JSX, type ReactNode } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  copyableAddressFromDaemonRow,
  type DaemonHandleRow,
} from '@/lib/chat-session-tab'
import { SessionViewTabs } from './SessionViewTabs'
import { SessionViewChromeContext } from './sessionViewChrome'
import { useSessionViewTab } from './useSessionViewTab'
import type { SessionViewTab } from './sessionViewTab'

interface AgentSessionChromeProps {
  /** Sidecar handle (`sales/reviewer`) or pinned workspace handle. */
  title: string
  addr: string
  conversationId: string | null
  /** v2_session_map key for this PTY — refresh closes then remounts. */
  agentName: string
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
  onRefresh,
  children,
}: AgentSessionChromeProps): JSX.Element {
  const sessionKey = conversationId || addr || agentName
  const [viewTab, setViewTab] = useSessionViewTab(sessionKey)
  const [refreshing, setRefreshing] = useState(false)
  const [nonce, setNonce] = useState(0)

  const handleRefresh = useCallback(async () => {
    if (refreshing) return
    setRefreshing(true)
    try {
      if (onRefresh) {
        onRefresh()
      } else {
        await daemonCliPost('sessions/v2/close', { agent_name: agentName, force: true })
        setNonce((n) => n + 1)
      }
    } catch (e) {
      console.warn('[AgentSessionChrome] sidecar refresh failed:', e)
    } finally {
      setRefreshing(false)
    }
  }, [refreshing, onRefresh, agentName])

  return (
    <SessionViewChromeContext.Provider
      value={{ viewTab, overlayAddr: addr, conversationId }}
    >
      <div className="h-full flex flex-col min-h-0" data-testid="sidecar-session-chrome">
        <SidecarSessionHeader
          title={title}
          viewTab={viewTab}
          onViewTabChange={setViewTab}
          onRefresh={() => void handleRefresh()}
          refreshing={refreshing}
        />
        <div className="flex-1 min-h-0" data-testid="agent-session-terminal">
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
  onViewTabChange,
  onRefresh,
  refreshing,
}: {
  title: string
  viewTab: SessionViewTab
  onViewTabChange: (tab: SessionViewTab) => void
  onRefresh: () => void
  refreshing: boolean
}): JSX.Element {
  return (
    <div
      className="border-b border-[var(--color-border)] flex-shrink-0 px-3 flex items-stretch gap-2"
      data-testid="sidecar-session-header"
    >
        <SessionViewTabs value={viewTab} onChange={onViewTabChange} />
        <span
          className="py-2 text-xs font-semibold text-[var(--color-text-primary)] truncate min-w-0 flex items-center flex-shrink-0"
          data-testid="sidecar-session-title"
        >
          {title}
        </span>
        <div className="flex-1" />
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
  )
}

export function PinnedSessionBody({
  viewTab,
  addr,
  conversationId,
  children,
}: {
  viewTab: SessionViewTab
  addr: string
  conversationId: string | null
  children: ReactNode
}): JSX.Element {
  return (
    <SessionViewChromeContext.Provider
      value={{ viewTab, overlayAddr: addr, conversationId }}
    >
      <div className="flex-1 min-h-0" data-testid="agent-session-terminal">
        {children}
      </div>
    </SessionViewChromeContext.Provider>
  )
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
    void (async () => {
      try {
        const rows = await daemonCliGet<DaemonHandleRow[]>('sessions/list-for-workspace', {
          path: projectPath,
        })
        if (cancelled) return
        const list = Array.isArray(rows) ? rows : []
        const name = attachAgentName || `tab-${paneGroupId}`
        const row =
          list.find((r) => r.agentName === name) ||
          list.find((r) => r.agentName === `tab-${paneGroupId}`)
        const addr = row ? copyableAddressFromDaemonRow(row) : null
        setState({
          title: addr?.clipboard || '',
          addr: addr?.clipboard || '',
        })
      } catch {
        if (!cancelled) setState({ title: '', addr: '' })
      }
    })()
    return () => {
      cancelled = true
    }
  }, [projectPath, paneGroupId, attachAgentName])

  return state
}
