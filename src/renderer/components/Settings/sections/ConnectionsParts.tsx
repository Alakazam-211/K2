// Settings → Connected Servers: the small pieces its server list and its
// agent list share, so a row and a state message look the same in both.

import React from 'react'
import { Callout, cx } from '@/components/ui'
import { SETTING_LIST_ROW_HOVER, SETTING_ROW_HINT, SETTING_ROW_LABEL } from '../controls/SettingControls'

export type HealthTone = 'ok' | 'warn' | 'error' | 'neutral'

/** Which fact a row's second line states. They always come in this order
 *  (health, password, pairing, agents across servers, in use), so every row
 *  reads the same way. */
export type ServerStatusKind = 'health' | 'saved' | 'paired' | 'federation' | 'active'

/** How a fact reads: a health in its own colour (plain for up, amber for a
 *  problem, red for down), anything else quietly. */
export type FactTone = HealthTone | 'quiet'

const FACT_TONE: Record<FactTone, string> = {
  ok: 'text-[var(--color-text-primary)] font-medium',
  neutral: 'text-[var(--color-text-secondary)] font-medium',
  warn: 'text-[var(--color-status-warn-text)] font-medium',
  error: 'text-[var(--color-status-error-soft)] font-medium',
  quiet: 'text-[var(--color-text-secondary)]',
}

/** One fact on a row's second line, in words. The row's status dot carries
 *  the colour; the words always say the state. */
export function StatusFact({
  kind,
  tone,
  title,
  children,
}: {
  kind: ServerStatusKind
  tone: FactTone
  title?: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <span data-server-status={kind} data-tone={tone} title={title} className={cx('whitespace-nowrap', FACT_TONE[tone])}>
      {children}
    </span>
  )
}

/** A row's facts with a quiet dot between each, skipping empty ones. */
export function Facts({ children }: { children: React.ReactNode }): React.JSX.Element {
  const items = React.Children.toArray(children).filter(Boolean)
  return (
    <>
      {items.map((item, i) => (
        <React.Fragment key={i}>
          {i > 0 && <MetaDot />}
          {item}
        </React.Fragment>
      ))}
    </>
  )
}

/** A server's address on its first line, beside its name: quieter, in the
 *  theme's own text font (Terminal's is already the code face). */
export function ServerAddress({ lock, children }: { lock?: React.ReactNode; children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="type-hint text-[var(--color-text-secondary)] inline-flex items-center gap-1 min-w-0 ml-1">
      {lock}
      <span className="truncate">{children}</span>
    </span>
  )
}

const DOT_TONE: Record<HealthTone, string> = {
  ok: 'bg-[var(--color-status-ok)]',
  warn: 'bg-[var(--color-status-warn)]',
  error: 'bg-[var(--color-status-error)]',
  neutral: 'bg-[var(--color-neutral)]',
}

/** Words carry the health, so "ok" stays plain: only a problem gets colour. */
const WORD_TONE: Record<HealthTone, string> = {
  ok: 'text-[var(--color-text-primary)]',
  neutral: 'text-[var(--color-text-secondary)]',
  warn: 'text-[var(--color-status-warn-text)]',
  error: 'text-[var(--color-status-error-soft)]',
}

export function StatusDot({ tone }: { tone: HealthTone }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      data-health-dot={tone}
      className={cx('w-2.5 h-2.5 flex-shrink-0 rounded-[var(--radius-badge)]', DOT_TONE[tone])}
    />
  )
}

export function HealthWord({ tone, children }: { tone: HealthTone; children: React.ReactNode }): React.JSX.Element {
  return <span className={cx('font-medium whitespace-nowrap', WORD_TONE[tone])}>{children}</span>
}

/** A server or agent in a list: status dot, then two lines, then actions.
 *  The row answers the pointer with the shared hover band. */
export function ConnectionRow({
  mark,
  title,
  meta,
  actions,
  children,
  className,
  ...rest
}: {
  mark?: React.ReactNode
  title: React.ReactNode
  meta?: React.ReactNode
  actions?: React.ReactNode
  /** Below the row, lined up with its text: progress, results, errors. */
  children?: React.ReactNode
} & Omit<React.HTMLAttributes<HTMLDivElement>, 'title'>): React.JSX.Element {
  return (
    <div className={cx('relative px-4 py-3 min-w-0', SETTING_LIST_ROW_HOVER, className)} {...rest}>
      <div className="flex items-center gap-3 min-w-0">
        {mark}
        <div className="flex-1 min-w-0">
          <div className="type-name text-[var(--color-text-primary)] min-w-0 flex items-center gap-1.5">{title}</div>
          {meta && (
            <div className="type-hint text-[var(--color-text-secondary)] mt-0.5 min-w-0 flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
              {meta}
            </div>
          )}
        </div>
        {actions && <div className="flex items-center gap-2 flex-shrink-0">{actions}</div>}
      </div>
      {children && <div className={cx('mt-2 space-y-2', mark ? 'pl-[22px]' : undefined)}>{children}</div>}
    </div>
  )
}

/** Between facts on a row's second line. */
export function MetaDot(): React.JSX.Element {
  return <span aria-hidden="true">·</span>
}

/** A message that takes a row's place inside a list card (nothing saved
 *  yet, nothing matches): the same title and hint as a Settings load state. */
export function StateNote({
  title,
  hint,
  action,
  'data-testid': testId,
}: {
  title: React.ReactNode
  hint?: React.ReactNode
  action?: React.ReactNode
  'data-testid'?: string
}): React.JSX.Element {
  return (
    <div className="px-4 py-3 flex items-center justify-between gap-6" data-testid={testId}>
      <div className="min-w-0">
        <p className={SETTING_ROW_LABEL}>{title}</p>
        {hint && <p className={SETTING_ROW_HINT}>{hint}</p>}
      </div>
      {action && <div className="flex items-center gap-2 flex-shrink-0">{action}</div>}
    </div>
  )
}

/** A failed action, in a row or a dialog: what failed in plain words, what
 *  to do now, then the server's own reason, quietly. */
export function ProblemNote({
  what,
  next,
  detail,
  action,
  testId,
}: {
  what: string
  next?: React.ReactNode
  detail?: string | null
  action?: React.ReactNode
  testId?: string
}): React.JSX.Element {
  const reason = detail?.trim().replace(/[.\s]+$/, '')
  return (
    <Callout tone="error" role="alert" data-testid={testId} className="type-hint">
      <p className="font-medium text-[var(--color-status-error-soft)]">{what}</p>
      {next && <p className="text-[var(--color-text-primary)] mt-0.5 leading-relaxed">{next}</p>}
      {reason && <p className="text-[var(--color-text-secondary)] mt-0.5 leading-relaxed break-words">The server said: {reason}.</p>}
      {action && <div className="mt-2 flex items-center gap-2">{action}</div>}
    </Callout>
  )
}
