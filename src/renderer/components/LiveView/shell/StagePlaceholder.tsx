// Phase 1's hero: the view's stage (the picture's or the monitor's well),
// empty but for one quiet line, with the off-screen stacks on its edges.
// Later phases put the scene's picture and the timeline's monitor and lanes
// here; the shell around it doesn't change.

import { cx } from '@/components/ui'
import { EdgeStack } from './EdgeStack'
import type { ShellHeroProps } from './shell-config'

const EDGE_PLACE = {
  left: 'left-3 top-1/2 -translate-y-1/2',
  right: 'right-3 top-1/2 -translate-y-1/2',
  top: 'top-3 left-1/2 -translate-x-1/2',
  bottom: 'bottom-3 left-1/2 -translate-x-1/2',
} as const

export function StagePlaceholder({ kind, line, p, below }: { kind: 'scene' | 'timeline'; line: string; p: ShellHeroProps; below?: React.ReactNode }): React.JSX.Element {
  return (
    <div data-testid="lv-hero" data-kind={kind} data-state={p.state} data-jumped={p.jumped ?? undefined} className="flex min-h-0 flex-1 flex-col gap-2 p-2.5">
      <div data-k2-chamfer="surface" className={cx('lv-stage relative flex min-h-0 flex-1 items-center justify-center rounded-[var(--radius-box)]')}>
        <p className="type-row m-0 max-w-[44ch] px-6 text-center text-[var(--color-text-secondary)]">{line}</p>
        {p.offscreen.map((o) => (
          <EdgeStack key={o.side} side={o.side} people={o.people} onJump={p.onJump} className={cx('absolute z-10', EDGE_PLACE[o.side])} />
        ))}
      </div>
      {below}
    </div>
  )
}
