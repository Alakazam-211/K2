import { forwardRef } from 'react'
import { cx } from './cx'

/** Background role → contract slot. `solid` is the mandatory opaque surface —
 *  the only background allowed under WebGL/terminal content, and never
 *  translucent in any Style. */
const ROLE_BG: Record<string, string> = {
  canvas: 'bg-[var(--color-bg-canvas)]',
  solid: 'bg-[var(--color-bg)]',
  surface: 'bg-[var(--color-bg-surface)]',
  elevated: 'bg-[var(--color-bg-elevated)]',
  inset: 'bg-[var(--color-bg-inset)]',
  // Pane-chrome strip (PaneTabBar) — historically painted bg-stripe
  // directly; promoted to a role so the strip reads its slot through
  // the primitive (Glass paints these surfaces with material).
  stripe: 'bg-[var(--color-bg-stripe)]',
}

const ELEVATION_SHADOW: Record<number, string> = {
  0: '[box-shadow:var(--ring-surface)]',
  1: '[box-shadow:var(--ring-surface),var(--shadow-1)]',
  2: '[box-shadow:var(--ring-surface),var(--shadow-2)]',
  3: '[box-shadow:var(--ring-surface),var(--shadow-3)]',
  4: '[box-shadow:var(--ring-surface),var(--shadow-4)]',
  5: '[box-shadow:var(--ring-surface),var(--shadow-5)]',
}

export interface SurfaceProps extends React.HTMLAttributes<HTMLDivElement> {
  /** Which background slot to paint. Default 'surface'. */
  role2?: 'canvas' | 'solid' | 'surface' | 'elevated' | 'inset' | 'stripe'
  /** 1px border on all sides (Terminal's panel treatment). Default true. */
  bordered?: boolean
  /** Elevation 0-5 → composes ring + shadow slots into one box-shadow. Default 0. */
  elevation?: 0 | 1 | 2 | 3 | 4 | 5
  /** A card, tile or popover: cut corners in Terminal (and nothing elsewhere).
   *  Panes and other structure keep the default: square. */
  chamfer?: boolean
}

/** The panel/card primitive: background role + radius.box + ring/shadow
 *  composition. In Terminal this renders exactly the historical
 *  `bg-[var(--color-bg-surface)] border border-[var(--color-border)]`
 *  panel (radius 0, no-op ring/shadows). Other Styles restyle it through
 *  the contract without component changes. */
export const Surface = forwardRef<HTMLDivElement, SurfaceProps>(function Surface(
  { role2 = 'surface', bordered = true, elevation = 0, chamfer = false, className, ...rest },
  ref
) {
  return (
    <div
      ref={ref}
      data-surface={role2}
      data-k2-chamfer={chamfer ? 'surface' : undefined}
      className={cx(
        ROLE_BG[role2],
        bordered && 'border border-[var(--color-border)]',
        'rounded-[var(--radius-box)]',
        ELEVATION_SHADOW[elevation],
        !bordered && '[--k2-ch-bw:0px]',
        className
      )}
      {...rest}
    />
  )
})
