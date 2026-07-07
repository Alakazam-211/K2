// Composer compose bar docked beneath a terminal pane. The human types a
// draft (persisted per-session in the THIN CLIENT — renderer localStorage,
// per-user/per-device, NEVER the daemon which may be remote/shared — so it
// survives workspace/tab switches AND app crashes/restarts), hits
// Enter, and the message is delivered to the agent via the daemon route
// POST /cli/terminal/send-message (attributed `[from <name>] `, submitted
// once through the per-session injection lock). NOT raw keystrokes — raw TUI
// control (arrows, Ctrl-C, menu nav) still goes through typing in the
// terminal itself. Renderer-hide is gated by composerPermitted (1c); the
// daemon enforces the same gate server-side.
//
// Condensed UI: just the input + its placeholder hint — no title, no send
// button, no status lane, no collapse control. A successful send clears the
// box; a failed send restores the text (the box reappearing IS the feedback).

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import {
  type MsgResponse,
  composerPermitted,
  mapMsgResponseToStatus,
  shouldSendOnKey,
} from './terminalCompose'

const MAX_TEXTAREA_HEIGHT = 160 // px — auto-grow cap before internal scroll

interface TerminalComposeBarProps {
  /** Resolved PTY SessionId for this pane — the pane's `terminalId`. */
  sessionId: string
}

export function TerminalComposeBar({ sessionId }: TerminalComposeBarProps): React.JSX.Element | null {
  // 1c (D4) + #67 renderer-hide: shown iff the active host is LOCAL (owner)
  // OR the app-level master is on OR the ACTIVE WORKSPACE opted into remote
  // instruction (default OFF). The daemon enforces the same gate per-workspace
  // server-side; this hide is defense-in-depth only.
  const isLocalHost = useConnectHostStore((s) => s.activeHost === 'local')
  const allowRemoteInstruct = useSettingsStore((s) => s.allowRemoteInstruct)
  // Per-workspace opt-in for the currently-active workspace. Approximate on
  // the renderer (the daemon resolves the EXACT target session's workspace);
  // good enough for the convenience hide.
  const perWorkspaceAllow = useProjectsStore((s) => {
    const active = s.projects.find((p) => p.id === s.activeProjectId)
    return (active?.allowRemoteInstruct ?? 0) === 1
  })
  const permitted = composerPermitted({ isLocalHost, allowRemoteInstruct, perWorkspaceAllow })

  // Draft persistence (thin client): key the draft by this pane's PTY session
  // and back it with localStorage so switching workspaces/tabs restores each
  // composer's own text instead of clearing it, and a crash/restart never loses
  // it. Per-user/per-device by nature (localStorage is the desktop app's own
  // storage); the draft never touches the daemon.
  const draftKey = `k2:composer:draft:${sessionId}`
  const [draft, setDraft] = useState<string>(() => {
    try {
      return localStorage.getItem(`k2:composer:draft:${sessionId}`) ?? ''
    } catch {
      return ''
    }
  })
  const [sending, setSending] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Reload the saved draft when the pane's session changes — this component can
  // be reused with a new sessionId on a workspace/tab switch without remounting.
  useEffect(() => {
    try {
      setDraft(localStorage.getItem(draftKey) ?? '')
    } catch {
      setDraft('')
    }
  }, [draftKey])

  // Persist on every change (localStorage writes are cheap + synchronous — this
  // is the crash-durable store). An empty draft clears the key.
  useEffect(() => {
    try {
      if (draft) localStorage.setItem(draftKey, draft)
      else localStorage.removeItem(draftKey)
    } catch {
      /* storage disabled/full — draft just won't persist */
    }
  }, [draft, draftKey])

  // Auto-grow the textarea to fit its content, capped at MAX_TEXTAREA_HEIGHT.
  const autoGrow = useCallback(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`
  }, [])

  useEffect(() => {
    autoGrow()
  }, [draft, autoGrow])

  const send = useCallback(async () => {
    const text = draft.trim()
    if (!text || sending) return

    setSending(true)
    setDraft('') // optimistic clear (PRD 1b); restored below only on failure

    try {
      const resp = await daemonCliPost<MsgResponse>('terminal/send-message', {
        session_id: sessionId,
        text,
      })
      // Failed send → restore the text so it's not lost (the box reappearing
      // IS the feedback) — but never clobber a fresh draft already started.
      if (mapMsgResponseToStatus(resp).kind !== 'delivered') {
        setDraft((cur) => (cur.length === 0 ? text : cur))
      }
    } catch {
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
      // Stop terminal-level / global single-key shortcuts from firing while
      // the human is typing a message in the box.
      e.stopPropagation()
    },
    [send]
  )

  // 1c renderer-hide (after all hooks): not permitted → render nothing.
  if (!permitted) return null

  // One condensed row — just the textarea.
  return (
    <div
      className="flex flex-shrink-0 items-start gap-1 border-t border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-2 pt-1.5 pb-2.5"
      data-compose-bar=""
      data-session-id={sessionId}
    >
      <textarea
        ref={textareaRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={handleKeyDown}
        rows={1}
        spellCheck={false}
        placeholder="Message the agent — Enter to send, Shift+Enter for newline"
        className="flex-1 resize-none bg-[var(--color-bg)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
        style={{
          fontFamily:
            "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
          fontSize: 12,
          lineHeight: 1.4,
          border: '1px solid var(--color-border)',
          borderRadius: 0,
          padding: '4px 6px',
          maxHeight: MAX_TEXTAREA_HEIGHT,
          overflowY: 'auto',
        }}
      />
    </div>
  )
}
