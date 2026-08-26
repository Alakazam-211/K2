import type { JSX } from 'react'
import type { SessionViewTab } from './sessionViewTab'

interface SessionViewTabsProps {
  value: SessionViewTab
  onChange: (tab: SessionViewTab) => void
}

const TAB_CLASS = (active: boolean) =>
  `px-2.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer inline-flex items-center ${
    active
      ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
      : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
  }`

/** Feedback-style underline tabs. Order: Terminal | Thread | split. */
export function SessionViewTabs({ value, onChange }: SessionViewTabsProps): JSX.Element {
  return (
    <div
      className="flex items-stretch gap-1 self-stretch flex-shrink-0"
      data-testid="session-view-tabs"
      role="tablist"
    >
      <button
        type="button"
        role="tab"
        aria-selected={value === 'terminal'}
        data-testid="session-view-tab-terminal"
        onClick={() => onChange('terminal')}
        className={TAB_CLASS(value === 'terminal')}
      >
        Terminal
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={value === 'thread'}
        data-testid="session-view-tab-thread"
        onClick={() => onChange('thread')}
        className={TAB_CLASS(value === 'thread')}
      >
        Thread
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={value === 'split'}
        data-testid="session-view-tab-split"
        aria-label="Split Terminal and Thread"
        title="Split — Terminal left, Thread right"
        onClick={() => onChange('split')}
        className={TAB_CLASS(value === 'split')}
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
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="16" rx="1.5" />
          <line x1="12" y1="4" x2="12" y2="20" />
        </svg>
      </button>
    </div>
  )
}
