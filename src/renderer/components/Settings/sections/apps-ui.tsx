// Settings → Apps: the small pieces every panel of the page shares, so the
// rail, This server, Host and each App read as one page (the Gardens and
// Workspaces layout: a list on the left, a pane of card groups on the right).

import React from 'react'
import { Callout } from '@/components/ui'
import { SETTING_LIST_ROW_HOVER, SETTING_ROW_HINT } from '../controls/SettingControls'

/** The pane's reading width, as Gardens'. */
export const APPS_CONTENT_WIDTH = 'max-w-[52rem]'

/** A list row in the rail: the name over one line of detail. Selected: the
 *  accent wash and bar; keyboard focus is the list rows' ring. */
export const APPS_LIST_ROW =
  'relative w-full flex items-center gap-3 text-left px-3 py-2 no-drag cursor-pointer select-none outline-none transition-colors'
export const APPS_ROW_ON = 'bg-[var(--color-accent)]/15'
export const APPS_ROW_OFF = SETTING_LIST_ROW_HOVER

export function SelectedBar(): React.JSX.Element {
  return <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />
}

/** The pane's title, its one-line lead and its actions on the right. */
export function PaneHeader({
  title,
  hint,
  actions,
  testId,
}: {
  title: React.ReactNode
  hint?: React.ReactNode
  actions?: React.ReactNode
  testId?: string
}): React.JSX.Element {
  return (
    <header className="flex items-start justify-between gap-4 min-w-0" data-testid={testId}>
      <div className="min-w-0 flex-1">
        <h2 className="type-title text-[var(--color-text-primary)] truncate">{title}</h2>
        {hint ? <div className={SETTING_ROW_HINT}>{hint}</div> : null}
      </div>
      {actions ? <div className="flex items-center gap-2 flex-shrink-0">{actions}</div> : null}
    </header>
  )
}

export type DotTone = 'ok' | 'warn' | 'error' | 'idle'

const DOT: Record<DotTone, string> = {
  ok: 'bg-[var(--color-status-ok)]',
  warn: 'bg-[var(--color-status-warn-amber)]',
  error: 'bg-[var(--color-status-error)]',
  idle: 'bg-[var(--color-text-muted)]',
}

/** A status dot (round in every look: a status mark, not a control). */
export function StatusDot({ tone, className = '' }: { tone: DotTone; className?: string }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      data-status-dot={tone}
      className={`inline-block h-2 w-2 flex-shrink-0 rounded-[var(--radius-status)] ${DOT[tone]} ${className}`}
    />
  )
}

/** An App's running state in plain words, with its dot. */
export function appStateWords(state: string): { tone: DotTone; words: string } {
  switch (state) {
    case 'running':
      return { tone: 'ok', words: 'Running' }
    case 'starting':
    case 'installing':
      return { tone: 'warn', words: state === 'starting' ? 'Starting' : 'Installing' }
    case 'stopped':
    case 'exited':
      return { tone: 'idle', words: 'Stopped' }
    case 'parked':
      return { tone: 'idle', words: 'Parked' }
    case 'waiting_engine':
      return { tone: 'warn', words: 'Waiting for the App engine' }
    case 'crashlooping':
      return { tone: 'error', words: 'Keeps crashing' }
    case 'unhealthy':
      return { tone: 'error', words: 'Not healthy' }
    case 'failed':
      return { tone: 'error', words: 'Failed' }
    default:
      return { tone: 'idle', words: state ? state.charAt(0).toUpperCase() + state.slice(1).replace(/_/g, ' ') : 'Unknown' }
  }
}

/** A square monogram for an App with no agent picture: its first letter on
 *  the field ground (cut in Terminal, rounded in Glass by the field radius). */
export function AppMonogram({ name, size = 24 }: { name: string; size?: number }): React.JSX.Element {
  const letter = (name.match(/[a-z0-9]/i)?.[0] ?? '?').toUpperCase()
  return (
    <span
      aria-hidden="true"
      data-app-monogram=""
      style={{ width: size, height: size }}
      className="flex-shrink-0 inline-flex items-center justify-center rounded-[var(--radius-field)] border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] type-hint font-medium text-[var(--color-text-secondary)]"
    >
      {letter}
    </span>
  )
}

/** The one line a Linux-only part of Apps shows on a Mac. Local for now;
 *  the Cores pages share one helper for it. */
export function LinuxOnlyNote({ children, testId }: { children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <Callout tone="info" className="type-hint" data-testid={testId}>
      {children}
    </Callout>
  )
}

/** A text link styled like the sentence around it, with the shared focus ring. */
export function LinkButton({
  children,
  onClick,
  testId,
}: {
  children: React.ReactNode
  onClick: () => void
  testId?: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-k2-focus=""
      data-testid={testId}
      onClick={onClick}
      className="relative type-hint text-[var(--color-accent-text)] underline underline-offset-2 hover:text-[var(--color-text-primary)] no-drag cursor-pointer outline-none"
    >
      {children}
    </button>
  )
}
