import type { JSX } from 'react'
import type { SessionViewTab } from './sessionViewTab'

interface SessionViewTabsProps {
  value: SessionViewTab
  onChange: (tab: SessionViewTab) => void
}

/** Feedback-style underline tabs (C1). Labels Thread | Terminal (C2). */
export function SessionViewTabs({ value, onChange }: SessionViewTabsProps): JSX.Element {
  return (
    <div className="flex items-center gap-1" data-testid="session-view-tabs" role="tablist">
      {(['thread', 'terminal'] as const).map((t) => (
        <button
          key={t}
          type="button"
          role="tab"
          aria-selected={value === t}
          data-testid={`session-view-tab-${t}`}
          onClick={() => onChange(t)}
          className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
            value === t
              ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
              : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
          }`}
        >
          {t === 'thread' ? 'Thread' : 'Terminal'}
        </button>
      ))}
    </div>
  )
}
