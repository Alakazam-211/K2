// The left nav's panel, on every page with one (Home, Agents, Projects): a
// Surface with Glass's tile material, so Glass draws it as one rounded tile
// with its rim, and Terminal as the plain surface panel with its right edge.
// The page around it sets the window inset and the pane gap to the main
// area; the column inside lays itself out and scrolls its own parts.

import { forwardRef } from 'react'
import { Surface } from '../ui'
import { cx } from '../ui/cx'

/** The tile's own classes (width comes from the page). */
export const NAV_TILE_CLASS = 'relative flex-shrink-0 overflow-y-auto border-r border-[var(--color-border)]'

export const NavTile = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(function NavTile(
  { className, ...rest },
  ref,
) {
  return (
    <Surface
      ref={ref}
      role2="surface"
      material="tile"
      bordered={false}
      data-nav-tile=""
      className={cx(NAV_TILE_CLASS, className)}
      {...rest}
    />
  )
})
