// What the stats line adds on the model path: how many items aren't drawn
// (with why, in plain words, in a tooltip on hover or keyboard focus), and
// the paper layouts, listed as coming next. Also the source the viewer reads
// the model from: the one passed in, else the daemon's for this file.

import { useId, useMemo } from 'react'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import type { NotDrawn } from './load'
import { daemonDrawingSource, type DrawingModelSource } from './source'
import type { ModelLayout } from './types'

export function useModelSource(given: DrawingModelSource | null | undefined, path: string | undefined): DrawingModelSource | null {
  const scope = useOptionalRoom()?.scope ?? null
  // One source per file and server, kept across versions (its block definitions are cached by digest).
  const daemon = useMemo(() => (scope && path ? daemonDrawingSource(scope, path) : null), [scope, path])
  return given === undefined ? daemon : given
}

export function NotDrawnNote({ nd }: { nd: NotDrawn }): React.JSX.Element {
  const id = `dxf-why-${useId().replace(/[^a-zA-Z0-9]/g, '')}`
  return (
    <span
      tabIndex={0}
      data-k2-focus=""
      aria-describedby={id}
      data-testid="dxf-not-drawn"
      className="group relative shrink-0 cursor-help underline decoration-dotted underline-offset-2"
    >
      {`${nd.count} ${nd.count === 1 ? 'item' : 'items'} not drawn`}
      <span
        id={id}
        role="tooltip"
        data-testid="dxf-not-drawn-why"
        className="invisible absolute right-0 top-full z-20 mt-1 w-max max-w-[360px] whitespace-normal rounded-[var(--radius-box)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-2.5 py-1.5 text-[11px] leading-snug text-[var(--color-text-primary)] no-underline group-hover:visible group-focus:visible"
      >
        {nd.lines.map((l) => (
          <span key={l} className="block">
            {l}
          </span>
        ))}
      </span>
    </span>
  )
}

export function LayoutsNote({ paper }: { paper: readonly ModelLayout[] }): React.JSX.Element {
  const names = paper.map((l, i) => l.name || `Layout ${l.tab ?? i + 1}`)
  return (
    <span data-testid="dxf-layouts" title={`Model space is shown. Paper layouts (${names.join(', ')}) are coming next.`}>
      {` · ${names.join(', ')} coming next`}
    </span>
  )
}
