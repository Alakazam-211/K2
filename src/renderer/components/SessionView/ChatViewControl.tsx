import type { JSX } from 'react'
import type { SessionViewTab } from './sessionViewTab'

/**
 * Temporary Chat selector for phase 1.
 * Not a fifth underline tab — `SessionViewTabs` stays Terminal | Thread | Chatter | split.
 * The phase-2 View menu replaces this control. Enabled only for Claude, Codex, Grok, and Gemini.
 * A shell pane has no session chrome, so it never renders this button.
 */
export function ChatViewControl({
  value,
  eligible,
  onChange,
}: {
  value: SessionViewTab
  eligible: boolean
  onChange: (tab: SessionViewTab) => void
}): JSX.Element {
  const active = value === 'chat' && eligible
  return (
    <button
      type="button"
      data-testid="session-view-chat"
      aria-pressed={active}
      aria-disabled={!eligible}
      disabled={!eligible}
      title={
        eligible
          ? 'Show the session log as chat. The terminal keeps running.'
          : 'Chat is available for Claude, Codex, Grok, and Gemini'
      }
      onClick={() => {
        if (!eligible) return
        onChange('chat')
      }}
      className={`self-center px-2 py-0.5 text-[11px] font-medium rounded flex-shrink-0 ${
        active
          ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-muted)]'
      } ${
        eligible
          ? 'cursor-pointer hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
          : 'opacity-40 cursor-not-allowed'
      }`}
    >
      Chat
    </button>
  )
}
