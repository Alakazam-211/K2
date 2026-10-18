// AgentListStatus — the one-line status a `SearchableAgentList` section
// shows under its header (Loading…, Offline + Retry, Sign in, an error),
// shared by Home's Add Agent picker and the workspace Connections picker
// (0.46.1) so the two say things the same way. Theme tokens only: the
// hover washes work on light palettes too.

import type React from 'react'

export const agentListStatusClass = 'px-2 py-1.5 text-[11px] text-[var(--color-text-muted)]'

export const agentListStatusButtonClass =
  'no-drag px-2 py-0.5 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] cursor-pointer flex-shrink-0'

const primaryButtonClass =
  'no-drag px-2 py-0.5 text-[11px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 cursor-pointer flex-shrink-0'

const toneClass = {
  muted: 'text-[var(--color-text-muted)]',
  secondary: 'text-[var(--color-text-secondary)]',
  error: 'text-[var(--color-status-error-soft)]',
} as const

export interface AgentListStatusProps {
  text: React.ReactNode
  tone?: keyof typeof toneClass
  /** A button at the line's end (Retry, Sign in). */
  action?: { label: string; onClick: () => void; primary?: boolean; disabled?: boolean }
  /** `data-listing` value, for tests and styling hooks. */
  marker?: string
}

export function AgentListStatus({ text, tone = 'muted', action, marker }: AgentListStatusProps): React.JSX.Element {
  if (!action) {
    return (
      <p className={tone === 'muted' ? agentListStatusClass : `px-2 py-1.5 text-[11px] ${toneClass[tone]}`} data-listing={marker}>
        {text}
      </p>
    )
  }
  return (
    <div className="flex items-center gap-2 px-2 py-1.5" data-listing={marker}>
      <p className={`flex-1 min-w-0 break-words text-[11px] ${toneClass[tone]}`}>{text}</p>
      <button
        type="button"
        className={action.primary ? primaryButtonClass : agentListStatusButtonClass}
        onClick={action.onClick}
        disabled={action.disabled}
      >
        {action.label}
      </button>
    </div>
  )
}
