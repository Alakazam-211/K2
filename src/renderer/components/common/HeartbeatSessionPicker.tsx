import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { ProviderIcon } from '@/components/AgentIcon/ProviderIcon'
import { useTabsStore } from '@/stores/tabs'
import {
  selectableSessions,
  type HeartbeatDeliveryMode,
  type HeartbeatDeliveryTarget,
  type HeartbeatSessionCandidate,
} from '@/lib/heartbeat-delivery'

/**
 * Per-heartbeat delivery drop-down — answers "where does this
 * heartbeat's wakeup go?". Replaces the "Send into pinned chat"
 * checkbox on the Settings heartbeat tiles (per-workspace
 * `HeartbeatsSection` and system-wide `WakeSchedulerSection`).
 *
 * Options, in order:
 *   1. "Pinned chat"  — the workspace's pinned chat session (the old
 *      checkbox's on-state). Prominent first entry.
 *   2. "Own session"  — the heartbeat's own session, new on next fire
 *      (the auto/default state).
 *   3. The workspace's saved sessions (any provider), EXCLUDING the
 *      session currently bound to the pinned chat — that one is only
 *      reachable via option 1 (see `selectableSessions`).
 *
 * Visuals copy AgentChatPane's `ChatHeader` history listbox (same
 * popover/option classes, ProviderIcon marks, message-count badge)
 * so mixed-agent session lists scan identically everywhere.
 *
 * Candidates are fetched lazily when the popover opens (never in a
 * render path): `chat/list` for the sessions + `workspace_session_get`
 * for the pinned-chat exclusion. When the closed trigger needs an
 * explicit session's title, `chat/list` also loads once on mount.
 */

interface HeartbeatSessionPickerProps {
  projectPath: string
  /** Canonical workspace id — keys the `workspace_session_get` read
   *  that resolves the pinned chat's session id for exclusion. */
  projectId: string
  value: HeartbeatDeliveryTarget
  onSelect: (next: HeartbeatDeliveryTarget) => void
}

export function HeartbeatSessionPicker({
  projectPath,
  projectId,
  value,
  onSelect,
}: HeartbeatSessionPickerProps): React.JSX.Element {
  const [open, setOpen] = useState(false)
  // null = not fetched yet (distinct from an empty workspace).
  const [sessions, setSessions] = useState<HeartbeatSessionCandidate[] | null>(null)
  // 0.40.48 host-aware fix: pinned-session ids now come from the ACTIVE
  // host's `chat/pinned` (global-scoped, same route ChatHistory uses) —
  // the old `workspace_session_get` invoke read THIS Mac's DB, so against
  // a remote host the exclusion keyed on the wrong machine. The single
  // excluded id is derived by intersecting with this project's candidates
  // (only this workspace's own pinned session can appear among them), so
  // `selectableSessions`' tested contract is unchanged.
  const [pinnedIds, setPinnedIds] = useState<string[] | null>(null)
  const [pinnedResolved, setPinnedResolved] = useState(false)
  const pinnedSessionId = useMemo<string | null>(() => {
    if (!pinnedIds || !sessions) return null
    return sessions.find((s) => pinnedIds.includes(s.sessionId))?.sessionId ?? null
  }, [pinnedIds, sessions])
  const rootRef = useRef<HTMLDivElement | null>(null)

  // The closed trigger shows the explicit session's TITLE, which only
  // chat/list knows — so that one case also warrants a mount fetch.
  const needTitle = value.mode === 'session' && !!value.sessionId

  useEffect(() => {
    if (!open && !(needTitle && sessions === null)) return
    let cancelled = false
    void daemonCliGet<Array<{
      sessionId: string
      title: string
      timestamp: number
      messageCount: number
      provider?: string
    }>>('chat/list', { project_path: projectPath })
      .then((rows) => {
        if (cancelled) return
        // Rows missing a provider (older daemons) degrade to "claude",
        // matching ChatHeader's normalization.
        setSessions(rows.map((r) => ({ ...r, provider: r.provider || 'claude' })))
      })
      .catch((err) => {
        console.warn('[HeartbeatSessionPicker] chat/list failed:', err)
      })
    return () => { cancelled = true }
    // `sessions` is deliberately NOT a dep — the effect re-runs on
    // open-toggles (refresh candidates like ChatHeader does), not on
    // its own fetch landing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, needTitle, projectPath])

  // Pinned-chat session ids — rows that must never be offered as normal
  // options. Resolved when the popover opens; session rows hold until it
  // lands so the pinned session can't flash into the list. Host-aware
  // (0.40.48): `chat/pinned` answers from the ACTIVE host, local or remote.
  useEffect(() => {
    if (!open) return
    let cancelled = false
    void daemonCliGet<string[]>('chat/pinned')
      .then((ids) => {
        if (cancelled) return
        setPinnedIds(ids)
        setPinnedResolved(true)
      })
      .catch((err) => {
        if (cancelled) return
        // No pinned rows (or read failure) = no pinned session to hide.
        console.warn('[HeartbeatSessionPicker] chat/pinned failed:', err)
        setPinnedIds([])
        setPinnedResolved(true)
      })
    return () => { cancelled = true }
  }, [open, projectId])

  // Close on outside click — the tiles render many pickers side by
  // side, so a stuck-open popover would stack under its neighbors.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    window.addEventListener('mousedown', onDown)
    return () => window.removeEventListener('mousedown', onDown)
  }, [open])

  const candidates = useMemo(
    () => selectableSessions(sessions ?? [], pinnedSessionId),
    [sessions, pinnedSessionId],
  )

  const label = useMemo<string>(() => {
    if (value.mode === 'pinned') return 'Pinned chat'
    if (value.mode === 'session') {
      const found = sessions?.find((s) => s.sessionId === value.sessionId)
      return found?.title || 'Saved session'
    }
    return 'Own session'
  }, [value.mode, value.sessionId, sessions])

  const pick = (next: HeartbeatDeliveryTarget): void => {
    setOpen(false)
    onSelect(next)
  }

  const listReady = sessions !== null && pinnedResolved

  return (
    <div ref={rootRef} className="relative inline-flex min-w-0">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        title="Where this heartbeat's wakeup is delivered — the workspace's pinned chat, its own session, or a saved session"
        aria-label="Heartbeat wakeup destination"
        aria-haspopup="listbox"
        aria-expanded={open}
        className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer min-w-0"
      >
        <span className="truncate max-w-[24ch]">{label}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
          <path d={open ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
        </svg>
      </button>
      {open && (
        <div
          role="listbox"
          className="absolute left-0 top-full mt-1 z-20 w-[36ch] max-h-[40vh] overflow-y-auto bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-2xl py-1"
        >
          {/* 1 — Pinned chat: the prominent, visually-distinct first
              entry (accent + pin mark, separated from the rest). */}
          <button
            type="button"
            role="option"
            aria-selected={value.mode === 'pinned'}
            onClick={() => pick({ mode: 'pinned' })}
            className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 border-b border-[var(--color-border)] font-medium transition-colors no-drag cursor-pointer ${
              value.mode === 'pinned'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)]'
                : 'text-[var(--color-accent)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
              <path d="M12 17v5" />
              <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1z" />
            </svg>
            <span className="flex-1 truncate">Pinned chat</span>
            {value.mode === 'pinned' && (
              <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0 text-[var(--color-accent)]">
                <path d="M5 12l5 5 9-11" />
              </svg>
            )}
          </button>
          {/* 2 — Own session: the auto/default state. */}
          <button
            type="button"
            role="option"
            aria-selected={value.mode === 'auto'}
            onClick={() => pick({ mode: 'auto' })}
            className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 transition-colors no-drag cursor-pointer ${
              value.mode === 'auto'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            <span className="flex-1 truncate">
              Own session
              <span className="text-[var(--color-text-muted)]"> (new on next fire)</span>
            </span>
            {value.mode === 'auto' && (
              <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0 text-[var(--color-accent)]">
                <path d="M5 12l5 5 9-11" />
              </svg>
            )}
          </button>
          {/* 3 — the workspace's saved sessions (pinned-chat session
              excluded — reachable only via option 1). */}
          {!listReady ? (
            <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
              Loading sessions…
            </div>
          ) : candidates.length === 0 ? (
            <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
              No past sessions yet.
            </div>
          ) : (
            candidates.map((s) => {
              const isCurrent = value.mode === 'session' && s.sessionId === value.sessionId
              return (
                <button
                  key={`${s.provider}:${s.sessionId}`}
                  type="button"
                  role="option"
                  aria-selected={isCurrent}
                  onClick={() => pick({ mode: 'session', sessionId: s.sessionId, provider: s.provider })}
                  className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 transition-colors no-drag cursor-pointer ${
                    isCurrent
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                  }`}
                >
                  {/* Provider mark so mixed-agent lists scan. */}
                  <ProviderIcon provider={s.provider} size={12} />
                  <span className="flex-1 truncate">{s.title || 'Untitled chat'}</span>
                  <span className="flex-shrink-0 text-[9px] text-[var(--color-text-muted)] opacity-70">
                    {s.messageCount}
                  </span>
                  {isCurrent && (
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0 text-[var(--color-accent)]">
                      <path d="M5 12l5 5 9-11" />
                    </svg>
                  )}
                </button>
              )
            })
          )}
        </div>
      )}
    </div>
  )
}

/**
 * Open the heartbeat's current delivery target as a tab:
 *
 *   - `pinned` → the workspace's pinned/agent-chat tab, via the tabs
 *     store's existing `openAgentPane` (which redirects to the pinned
 *     system-agent tab). The primary agent name is resolved the same
 *     way `openHeartbeatTab`'s surface path resolves it.
 *   - `auto` / `session` → the heartbeat's saved-session tab via
 *     `openHeartbeatTab` — the exact flow of the sidebar drawer's row
 *     click (focus live PTY, spawn-and-resume otherwise).
 */
export async function openHeartbeatTarget(
  projectPath: string,
  heartbeatName: string,
  mode: HeartbeatDeliveryMode,
): Promise<void> {
  const tabs = useTabsStore.getState()
  if (mode === 'pinned') {
    const agents = await invoke<Array<{ name: string; agentType: string }>>(
      'k2so_agents_list',
      { projectPath },
    ).catch(() => [] as Array<{ name: string; agentType: string }>)
    const agentName = agents.find((a) =>
      a.agentType === 'custom' || a.agentType === 'manager' || a.agentType === 'k2so',
    )?.name ?? agents[0]?.name ?? null
    if (!agentName) {
      console.warn('[HeartbeatSessionPicker] no agent resolved for pinned-chat open:', projectPath)
      return
    }
    tabs.openAgentPane(agentName, projectPath)
    return
  }
  await tabs.openHeartbeatTab(projectPath, heartbeatName)
}
