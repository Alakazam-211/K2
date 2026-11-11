// The state inside an agent or document tile on the Projects page, and the
// quiet Try again every failure on the page offers. Kept apart from
// ProjectStates.tsx so the tiles import no page code.

import React from 'react'
import { Button, Spinner } from '@/components/ui'

/** The quiet "Try again" every failure offers. */
export function TryAgain({ onClick, busy = false }: { onClick: () => void; busy?: boolean }): React.JSX.Element {
  return (
    <Button variant="quiet" onClick={onClick} disabled={busy}>
      Try again
    </Button>
  )
}

/** A state inside an agent or document tile: one plain sentence, an
 *  optional line, the server's reason quietly, and the next step, centred. */
export function TileState({
  title,
  children,
  detail,
  action,
  busy = false,
  error = false,
  testId,
}: {
  title: string
  children?: React.ReactNode
  detail?: string | null
  action?: React.ReactNode
  /** Checking or waking: a spinner and the quiet colour. */
  busy?: boolean
  error?: boolean
  testId?: string
}): React.JSX.Element {
  const reason = detail?.trim().replace(/[.\s]+$/, '')
  return (
    <div
      className="h-full flex flex-col items-center justify-center gap-1 px-4 text-center"
      role={error ? 'alert' : 'status'}
      aria-busy={busy || undefined}
      data-tile-state={error ? 'error' : busy ? 'busy' : 'idle'}
      data-testid={testId}
    >
      <p
        className={`type-row max-w-[40ch] flex items-center gap-2 ${
          busy ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'
        }`}
      >
        {busy && <Spinner />}
        <span>{title}</span>
      </p>
      {children && <p className="type-hint text-[var(--color-text-secondary)] max-w-[40ch]">{children}</p>}
      {reason && (
        <p
          data-load-detail=""
          title="What the server said"
          className="mt-1 max-w-[40ch] px-2.5 py-1 type-hint font-mono text-[var(--color-text-muted)] bg-[var(--color-wash-1)] rounded-[var(--radius-field)] break-words"
        >
          {reason}
        </p>
      )}
      {action && <div className="mt-2 flex items-center gap-2">{action}</div>}
    </div>
  )
}
