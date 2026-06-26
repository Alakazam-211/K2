// Composer Phase 1b — the renderer compose bar docked beneath a terminal
// pane. The human types a draft (renderer-local, ephemeral RAM only — NO
// daemon draft store, a PRD non-goal), hits Enter, and the message is
// delivered to the agent's TUI as one clean, attributed, verified
// injection via the Phase-1a daemon route `POST /cli/terminal/send-message`.
//
// Scope (PRD §Phasing 1b + D3/D6):
//  - This sends an attributed *message* to the agent (the daemon stamps a
//    `[from owner] ` prefix and submits it once through the per-session
//    injection lock). It is NOT raw keystrokes — raw TUI control (arrows,
//    Ctrl-C, menu nav) still goes through typing in the terminal itself.
//  - Draft is per-pane renderer state, never persisted/synced.
//  - 1a is owner-token-only; the daemon resolves identity from the token,
//    never from this client. The capability gate + renderer-hide land in
//    1c — NOT here.
//
// The send goes through the existing host-aware `daemonCliPost` client so
// it is ciphertext-safe over K2 Connect (no hand-rolled fetch). The route
// needs the resolved PTY `session_id`, which for the renderer IS the
// pane's `terminalId` (the same id every `/cli/terminal/*` lifecycle route
// parses via `SessionId::parse`).

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import {
  type ComposeStatus,
  type MsgResponse,
  composerPermitted,
  mapMsgResponseToStatus,
  shouldSendOnKey,
} from './terminalCompose'

// ── Status lane labels ───────────────────────────────────────────────

function statusLabel(status: ComposeStatus): { text: string; color: string } | null {
  switch (status.kind) {
    case 'idle':
      return null
    case 'injecting':
      return { text: 'injecting…', color: 'var(--color-text-muted)' }
    case 'delivered':
      return { text: 'delivered ✓', color: '#8bdb81' }
    case 'pty_died':
      return { text: 'pty_died ⚠', color: '#ff6b6b' }
    case 'pty_stalled':
      return { text: 'pty_stalled ⚠ (try again)', color: '#e6b450' }
    case 'busy':
      return { text: 'busy ⚠', color: '#e6b450' }
    case 'error':
      return { text: `error ⚠ ${status.message}`, color: '#ff6b6b' }
  }
}

const MAX_TEXTAREA_HEIGHT = 160 // px — auto-grow cap before internal scroll
const DELIVERED_CLEAR_MS = 3000 // auto-fade the green "delivered ✓" lane

interface TerminalComposeBarProps {
  /** Resolved PTY SessionId for this pane — the pane's `terminalId`. */
  sessionId: string
}

/**
 * Collapsible compose bar beneath a terminal pane. First-cut Phase 1b:
 * the bar + send + status lane. No capability gate (1c), no raw-typing
 * collision guard (1c), no queue UI (Phase 2).
 */
export function TerminalComposeBar({ sessionId }: TerminalComposeBarProps): React.JSX.Element | null {
  // Composer 1c (D4) — renderer-hide. The composer is shown iff the active
  // host is LOCAL (owner, always allowed) OR that host opted into remote
  // instruction (`allowRemoteInstruct`, default OFF). The DAEMON enforces
  // the same gate server-side (403); this hide is defense-in-depth only.
  const isLocalHost = useConnectHostStore((s) => s.activeHost === 'local')
  const allowRemoteInstruct = useSettingsStore((s) => s.allowRemoteInstruct)
  const permitted = composerPermitted({ isLocalHost, allowRemoteInstruct })

  const [collapsed, setCollapsed] = useState(false)
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const [status, setStatus] = useState<ComposeStatus>({ kind: 'idle' })
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Auto-grow the textarea to fit its content, capped at MAX_TEXTAREA_HEIGHT.
  const autoGrow = useCallback(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`
  }, [])

  useEffect(() => {
    if (!collapsed) autoGrow()
  }, [draft, collapsed, autoGrow])

  // Auto-fade the "delivered ✓" lane so a successful send doesn't leave a
  // stale green badge. Failures persist until the next send so the human
  // can read them (and retry on pty_stalled).
  useEffect(() => {
    if (status.kind !== 'delivered') return
    const t = setTimeout(() => setStatus({ kind: 'idle' }), DELIVERED_CLEAR_MS)
    return () => clearTimeout(t)
  }, [status])

  const send = useCallback(async () => {
    const text = draft.trim()
    if (!text || sending) return

    setSending(true)
    setStatus({ kind: 'injecting' })
    // Optimistic clear (PRD 1b). On a hard failure we restore the text
    // below — but only if the box is still empty — so a "(try again)"
    // is actionable without retyping, while never clobbering a new draft.
    setDraft('')

    try {
      const resp = await daemonCliPost<MsgResponse>('terminal/send-message', {
        session_id: sessionId,
        text,
      })
      const next = mapMsgResponseToStatus(resp)
      setStatus(next)
      if (next.kind !== 'delivered') {
        setDraft((cur) => (cur.length === 0 ? text : cur))
      }
    } catch (e) {
      setStatus({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
      setDraft((cur) => (cur.length === 0 ? text : cur))
    } finally {
      setSending(false)
    }
  }, [draft, sending, sessionId])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (shouldSendOnKey({ key: e.key, shiftKey: e.shiftKey, isComposing: e.nativeEvent.isComposing })) {
        e.preventDefault()
        void send()
      }
      // Stop terminal-level / global single-key shortcuts from firing
      // while the human is typing a message in the box.
      e.stopPropagation()
    },
    [send]
  )

  const lane = statusLabel(status)

  // 1c renderer-hide (after all hooks, per the Rules of Hooks): not
  // permitted → render nothing. The daemon still enforces the gate.
  if (!permitted) return null

  // ── Collapsed: a thin toggle strip ──────────────────────────────────
  if (collapsed) {
    return (
      <div
        className="flex flex-shrink-0 items-center border-t border-[var(--color-border)] bg-[#111] px-2"
        style={{ height: 22, minHeight: 22 }}
      >
        <button
          type="button"
          onClick={() => setCollapsed(false)}
          className="flex items-center gap-1.5 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
          style={{ fontSize: 11 }}
          title="Show message composer"
        >
          <span style={{ fontSize: 9 }}>▴</span>
          <span>Message agent</span>
        </button>
      </div>
    )
  }

  // ── Expanded: textarea + status lane ────────────────────────────────
  return (
    <div
      className="flex flex-shrink-0 flex-col border-t border-[var(--color-border)] bg-[#111]"
      data-compose-bar=""
      data-session-id={sessionId}
    >
      {/* Header row: title + collapse toggle */}
      <div className="flex items-center justify-between px-2 pt-1">
        <span className="text-[var(--color-text-muted)]" style={{ fontSize: 10, letterSpacing: 0.3 }}>
          MESSAGE AGENT
        </span>
        <button
          type="button"
          onClick={() => setCollapsed(true)}
          className="text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
          style={{ fontSize: 9, lineHeight: 1 }}
          title="Hide message composer"
        >
          ▾
        </button>
      </div>

      {/* Textarea */}
      <div className="px-2 pb-1">
        <textarea
          ref={textareaRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={handleKeyDown}
          rows={1}
          spellCheck={false}
          placeholder="Message the agent — Enter to send, Shift+Enter for newline"
          className="w-full resize-none bg-[#0a0a0a] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
          style={{
            fontFamily:
              "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
            fontSize: 12,
            lineHeight: 1.4,
            border: '1px solid var(--color-border)',
            borderRadius: 4,
            padding: '4px 6px',
            maxHeight: MAX_TEXTAREA_HEIGHT,
            overflowY: 'auto',
          }}
        />
      </div>

      {/* Status lane + send hint */}
      <div className="flex items-center justify-between px-2 pb-1" style={{ minHeight: 16 }}>
        <span style={{ fontSize: 10, color: lane ? lane.color : 'transparent' }}>
          {lane ? lane.text : ' '}
        </span>
        <button
          type="button"
          onClick={() => void send()}
          disabled={sending || draft.trim().length === 0}
          className="text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40 disabled:hover:text-[var(--color-text-secondary)] transition-colors"
          style={{ fontSize: 11 }}
        >
          Send ⏎
        </button>
      </div>
    </div>
  )
}
