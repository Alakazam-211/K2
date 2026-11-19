// The Projects page's state card (the owner's mockup, mockup-projects-reorg-v1
// stateCard()): one centred card for a page-sized load, empty or failure
// state. An icon (a spinner while loading, a warning on a failure), a title
// that says what happened, one plain line, the server's reason quietly in
// its own inset, and the next step as the primary button. Terminal: square
// and chamfered; Glass: the opaque tile with a round icon well.

import React from 'react'
import { Button, Spinner, Surface } from '@/components/ui'

export type StateCardKind = 'loading' | 'error' | 'empty'

const ICON = { width: 20, height: 20, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', strokeWidth: 1.7, strokeLinecap: 'round', strokeLinejoin: 'round' } as const

export function ProjectsIcon(): React.JSX.Element {
  return (
    <svg aria-hidden="true" {...ICON}>
      <rect x="3.5" y="3.5" width="7" height="7" />
      <rect x="13.5" y="3.5" width="7" height="7" />
      <rect x="3.5" y="13.5" width="7" height="7" />
      <rect x="13.5" y="13.5" width="7" height="7" />
    </svg>
  )
}

function WarnIcon(): React.JSX.Element {
  return (
    <svg aria-hidden="true" {...ICON}>
      <path d="M12 3.5 21.5 20h-19z" />
      <path d="M12 10v4.5M12 17.2v.3" />
    </svg>
  )
}

/** The small icons inside the card's primary button. */
export function PlusGlyph(): React.JSX.Element {
  return (
    <svg aria-hidden="true" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round">
      <path d="M12 5v14M5 12h14" />
    </svg>
  )
}
export function RetryGlyph(): React.JSX.Element {
  return (
    <svg aria-hidden="true" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round">
      <path d="M20 12a8 8 0 1 1-2.34-5.66" />
      <path d="M20 4v4.5h-4.5" />
    </svg>
  )
}

/** The card's primary button: the shared accent Button with its glyph. */
export function StateAction({
  onClick,
  glyph,
  disabled,
  busy = false,
  children,
}: {
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void
  glyph?: React.ReactNode
  disabled?: boolean
  /** In flight: the shared Button's disabled look, a spinner for the glyph. */
  busy?: boolean
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <Button
      variant="accent"
      size="md"
      onClick={onClick}
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      className="inline-flex items-center gap-1.5"
    >
      {busy ? <Spinner /> : glyph}
      {children}
    </Button>
  )
}

/** "Try again" as the card's primary button; "Trying again…" while it runs. */
export function StateRetry({ onClick, busy = false }: { onClick: () => void; busy?: boolean }): React.JSX.Element {
  return (
    <StateAction onClick={onClick} glyph={<RetryGlyph />} busy={busy}>
      {busy ? 'Trying again…' : 'Try again'}
    </StateAction>
  )
}

/** The server's own reason, quietly, in its inset: the UI face (the code
 *  face in Terminal, the system font in Glass). */
export function StateReason({ reason }: { reason: string }): React.JSX.Element {
  return (
    <p
      data-load-detail=""
      data-k2-chamfer="chip"
      title="What the server said"
      className="mt-1 max-w-full px-2.5 py-1 type-hint text-[var(--color-text-secondary)] bg-[var(--color-wash-1)] rounded-[var(--radius-field)] break-words"
    >
      {reason}
    </p>
  )
}

/** Tidy a server's reason for the inset (no trailing full stop). */
export function tidyReason(detail: string | null | undefined): string {
  return detail?.trim().replace(/[.\s]+$/, '') ?? ''
}

export function ProjectStateCard({
  kind,
  title,
  children,
  detail,
  action,
  icon,
  testId,
}: {
  kind: StateCardKind
  title: string
  /** One plain line under the title. */
  children?: React.ReactNode
  /** The server's own reason, shown quietly in its inset. */
  detail?: string | null
  action?: React.ReactNode
  /** The empty state's icon (loading and error bring their own). */
  icon?: React.ReactNode
  testId?: string
}): React.JSX.Element {
  const reason = tidyReason(detail)
  const glyph = kind === 'loading' ? <Spinner /> : kind === 'error' ? <WarnIcon /> : icon
  return (
    <Surface
      chamfer
      material="tile"
      role={kind === 'error' ? 'alert' : 'status'}
      aria-busy={kind === 'loading' || undefined}
      data-load-state={kind}
      data-testid={testId}
      className="w-full max-w-[400px] px-7 pt-7 pb-6 flex flex-col items-center gap-2 text-center"
    >
      {glyph && (
        <span
          data-k2-chamfer="chip"
          className={`mb-1 w-11 h-11 flex items-center justify-center rounded-[var(--radius-pill)] bg-[var(--color-wash-1)] ${
            kind === 'error' ? 'text-[var(--color-status-warn-text)]' : 'text-[var(--color-text-secondary)]'
          }`}
        >
          {glyph}
        </span>
      )}
      <p className="type-heading text-[var(--color-text-primary)]">{title}</p>
      {children && <p className="type-row text-[var(--color-text-secondary)] max-w-[34ch]">{children}</p>}
      {reason && <StateReason reason={reason} />}
      {action && <div className="mt-2.5 w-full flex justify-center">{action}</div>}
    </Surface>
  )
}

/** A page-sized state, centred on the canvas (or in a column). */
export function StateCanvas({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <div className="flex-1 min-h-0 h-full flex items-center justify-center p-6">{children}</div>
}
