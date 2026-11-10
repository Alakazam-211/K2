// Settings → Connected Servers: the small pieces its server list and its
// agent list share, so a row, a section heading and a state message look the
// same in both.

import { cx, Tag, type TagTone } from '@/components/ui'

export type HealthTone = 'ok' | 'warn' | 'error' | 'neutral'

/** A health as a chip: green for up, red for down, amber for a problem,
 *  quiet for anything in between. */
export const HEALTH_TAG_TONE: Record<HealthTone, TagTone> = {
  ok: 'ok',
  warn: 'warn',
  error: 'danger',
  neutral: 'muted',
}

/** Which fact a server's status chip states. The chips always come in this
 *  order, so every row reads the same way. */
export type ServerStatusKind = 'health' | 'saved' | 'paired' | 'federation' | 'active'

/** One fact on a server's second line: a `Tag` with a small icon. The words
 *  always say the state; the colour only helps. */
export function StatusChip({
  kind,
  tone,
  icon,
  title,
  children,
}: {
  kind: ServerStatusKind
  tone: TagTone
  icon?: React.ReactNode
  title?: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <Tag tone={tone} title={title} data-server-status={kind} className="gap-1">
      {icon}
      {children}
    </Tag>
  )
}

/** A server's address on its first line, beside its name: quieter, in the
 *  monospace that addresses use elsewhere in Settings. */
export function ServerAddress({ lock, children }: { lock?: React.ReactNode; children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="type-hint font-mono text-[var(--color-text-secondary)] inline-flex items-center gap-1 min-w-0 ml-1">
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

/** One heading per section, with its single action on the right. */
export function SectionHeader({
  title,
  hint,
  action,
}: {
  title: React.ReactNode
  hint?: React.ReactNode
  action?: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="mb-3">
      <div className="flex items-center justify-between gap-4 min-h-[1.75rem]">
        <h3 className="type-heading text-[var(--color-text-primary)]">{title}</h3>
        {action}
      </div>
      {hint && <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">{hint}</p>}
    </div>
  )
}

/** A server or agent in a list: mark, then two lines, then actions. */
export function ConnectionRow({
  mark,
  title,
  meta,
  actions,
  children,
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
    <div className="px-4 py-3 min-w-0" {...rest}>
      <div className="flex items-center gap-3 min-w-0">
        {mark}
        <div className="flex-1 min-w-0">
          <div className="type-name text-[var(--color-text-primary)] min-w-0 flex items-center gap-1.5">{title}</div>
          {meta && (
            <div className="type-hint text-[var(--color-text-secondary)] mt-1 min-w-0 flex flex-wrap items-center gap-x-1.5 gap-y-1">
              {meta}
            </div>
          )}
        </div>
        {actions && <div className="flex items-center gap-2 flex-shrink-0">{actions}</div>}
      </div>
      {children && <div className={cx('mt-2 space-y-1.5', mark ? 'pl-[22px]' : undefined)}>{children}</div>}
    </div>
  )
}

/** Between facts on a row's second line. */
export function MetaDot(): React.JSX.Element {
  return <span aria-hidden="true">·</span>
}

/** Empty, loading and "couldn't load" messages: what happened and what next. */
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
    <div className="px-4 py-5 flex items-center justify-between gap-4" data-testid={testId}>
      <div className="min-w-0">
        <div className="type-row text-[var(--color-text-primary)]">{title}</div>
        {hint && <div className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">{hint}</div>}
      </div>
      {action && <div className="flex-shrink-0">{action}</div>}
    </div>
  )
}
