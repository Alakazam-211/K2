// The off-screen stack: people whose place is past an edge of the view (a
// scene's picture, the timeline's ruler, the editor's text), as one pill on
// that edge with an arrow and up to three avatars; it jumps there. The
// person you follow never gets one (the view is already on them). The view
// places it; this only draws it.

import { cx } from '@/components/ui'
import type { LivePerson } from '@/lib/live-view/shared/people'
import { EdgeArrow } from './glyphs'
import { LiveAvatar } from './LiveAvatar'

export type Edge = 'left' | 'right' | 'top' | 'bottom'

const SIDE_WORD: Record<Edge, string> = { left: 'to the left', right: 'to the right', top: 'above', bottom: 'below' }

export function EdgeStack({
  side,
  people,
  className,
  onJump,
}: {
  side: Edge
  people: readonly LivePerson[]
  className?: string
  onJump: (side: Edge, ids: string[]) => void
}): React.JSX.Element | null {
  if (people.length === 0) return null
  const names = people.map((p) => p.name).join(', ')
  const label = `${names}, off screen ${SIDE_WORD[side]}: go there`
  const arrowFirst = side === 'left' || side === 'top'
  return (
    <button
      type="button"
      data-k2-focus=""
      data-k2-chamfer="chip"
      data-testid={`lv-edge-${side}`}
      aria-label={label}
      title={label}
      onClick={() => onJump(side, people.map((p) => p.id))}
      className={cx(
        'lv-edge no-drag inline-flex h-[24px] cursor-pointer items-center gap-1 rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] px-1 text-[var(--color-text-secondary)]',
        className,
      )}
    >
      {arrowFirst ? <EdgeArrow side={side} /> : null}
      <span className="lv-edge-avs inline-flex items-center">
        {people.slice(0, 3).map((p) => (
          <LiveAvatar key={p.id} person={p} size={16} />
        ))}
      </span>
      {arrowFirst ? null : <EdgeArrow side={side} />}
    </button>
  )
}
