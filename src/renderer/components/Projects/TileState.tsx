// The state inside an agent or document tile on the Projects page, and the
// quiet Try again every tile failure offers. Kept apart from
// ProjectStates.tsx so the tiles import no page code.
//
// A tile state is the page's state card made compact (StateCard.tsx): a
// title, one plain line, the server's reason quietly, the next step. Every
// tile places it the same way, a fixed step under the tile's bar (the step
// gives way in a short tile), so tiles side by side show theirs level.

import React from 'react'
import { Button, Spinner, Surface } from '@/components/ui'
import { RetryGlyph, StateReason, tidyReason } from './StateCard'

/** The quiet "Try again" every tile failure offers; "Trying again…" while
 *  it runs (the shared Button's disabled look). */
export function TryAgain({ onClick, busy = false }: { onClick: () => void; busy?: boolean }): React.JSX.Element {
  return (
    <Button
      variant="quiet"
      size="md"
      onClick={onClick}
      disabled={busy}
      aria-busy={busy || undefined}
      className="inline-flex items-center gap-1.5"
    >
      {busy ? <Spinner /> : <RetryGlyph />}
      {busy ? 'Trying again…' : 'Try again'}
    </Button>
  )
}

/** A text link inside a tile state's line (the next step after Try again). */
export function TileStateLink({ onClick, children }: { onClick: () => void; children: React.ReactNode }): React.JSX.Element {
  return (
    <button
      type="button"
      data-k2-focus=""
      onClick={onClick}
      className="relative px-0.5 -mx-0.5 text-[var(--color-accent-text)] underline underline-offset-2 hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
    >
      {children}
    </button>
  )
}

/** A state inside an agent or document tile: the compact state card. */
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
  /** One plain line under the title. */
  children?: React.ReactNode
  /** The server's own reason, shown quietly in its inset. */
  detail?: string | null
  action?: React.ReactNode
  /** Checking, waking or opening: a spinner beside the title. */
  busy?: boolean
  error?: boolean
  testId?: string
}): React.JSX.Element {
  const reason = tidyReason(detail)
  return (
    <div className="h-full min-h-0 overflow-y-auto flex flex-col items-center px-3 pb-3" data-tile-state-host="">
      {/* The fixed step under the bar; it shrinks first in a short tile. */}
      <div aria-hidden="true" className="flex-[0_1_48px] min-h-3" />
      <Surface
        chamfer
        material="tile"
        role={error ? 'alert' : 'status'}
        aria-busy={busy || undefined}
        data-tile-state={error ? 'error' : busy ? 'busy' : 'idle'}
        data-testid={testId}
        className="flex-shrink-0 w-full max-w-[340px] px-5 pt-4 pb-4 flex flex-col items-center gap-1.5 text-center"
      >
        <p className="type-name font-semibold text-[var(--color-text-primary)] flex items-center justify-center gap-2 max-w-full">
          {busy && <Spinner />}
          <span className="min-w-0 break-words">{title}</span>
        </p>
        {children && <p className="type-hint text-[var(--color-text-secondary)] max-w-[36ch]">{children}</p>}
        {reason && <StateReason reason={reason} />}
        {action && <div className="mt-1.5 flex flex-wrap items-center justify-center gap-2">{action}</div>}
      </Surface>
    </div>
  )
}
