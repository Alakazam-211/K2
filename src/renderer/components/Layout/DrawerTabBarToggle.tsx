// Drawer toggles (0.46.0 UI), on every page and window with drawers, in
// place of the top bar's drawer buttons. Uses the nav's » / « glyph.
//
// - Closed drawer: its own slot at the far edge of the room-side strip,
//   e.g. FAR LEFT (left drawer, ») of the leftmost room column's tab bar,
//   or FAR RIGHT (right drawer, «) of the rightmost column's, after split /
//   remove column.
// - Open drawer: the button moves into that drawer's own tab strip or
//   header, at the end nearest the room (the drawer's inner edge), pointing
//   at the window edge it collapses to (« in a left drawer, » in a right).
//
// `DrawerTabBarToggle` drives the workspace drawers (per-window chrome,
// `usePanelsStore`); `DrawerToggleButton` is the same button for any
// other drawer (e.g. the Projects page's chat panel, the Tickets list).

import type { ReactNode } from 'react'
import { DoubleChevron } from '@/components/Sidebar/SidebarCollapseButton'
import { usePanelsStore } from '@/stores/panels'
import { useRoomTabBarDrawerToggle } from './drawer-toggles'

type Side = 'left' | 'right'

export function DrawerToggleButton({
  side,
  placement,
  onToggle,
  name = `${side} drawer`,
  testId,
  children,
}: {
  side: Side
  /** 'room': in the room-side strip (drawer closed). 'drawer': in the drawer's own strip (open). */
  placement: 'room' | 'drawer'
  onToggle: () => void
  /** What the tooltip calls it: "Show <name>" / "Hide <name>". */
  name?: string
  /** `data-testid` for the button (e.g. the Tickets list's collapse). */
  testId?: string
  /** Extra marks drawn on the button (e.g. an unread dot). */
  children?: ReactNode
}): React.JSX.Element {
  const opening = placement === 'room'
  // Opening points toward the room (where the drawer slides in from the
  // edge); closing points at the edge the drawer collapses to.
  const direction: Side = opening ? (side === 'left' ? 'right' : 'left') : side
  const label = `${opening ? 'Show' : 'Hide'} ${name}`
  return (
    <button
      type="button"
      onClick={() => onToggle()}
      title={label}
      aria-label={label}
      aria-expanded={!opening}
      data-testid={testId}
      data-drawer-tabbar-toggle={side}
      data-placement={placement}
      className="no-drag relative flex h-full w-8 flex-shrink-0 cursor-pointer items-center justify-center self-stretch bg-white/[0.04] text-[var(--color-text-secondary)] outline-none transition-colors hover:bg-white/[0.08] hover:text-[var(--color-text-primary)] focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[var(--color-accent)]"
    >
      <DoubleChevron direction={direction} className="h-4 w-4" />
      {children}
    </button>
  )
}

export default function DrawerTabBarToggle({
  side,
  placement,
}: {
  side: Side
  placement: 'room' | 'drawer'
}): React.JSX.Element {
  const toggle = usePanelsStore((s) => (side === 'left' ? s.toggleLeftPanel : s.toggleRightPanel))
  return <DrawerToggleButton side={side} placement={placement} onToggle={toggle} />
}

/** For a main area with no tab bar (no workspace yet, a Focus window still
 *  loading): a closed workspace drawer's toggle in the area's top corner,
 *  the same height as a tab bar, so a drawer is never out of reach. */
export function NoTabBarDrawerToggles(): React.JSX.Element {
  const left = useRoomTabBarDrawerToggle('left')
  const right = useRoomTabBarDrawerToggle('right')
  return (
    <>
      {left && (
        <div className="absolute left-0 top-0 z-10 flex h-9" data-no-tabbar-drawer-toggle="left">
          <DrawerTabBarToggle side="left" placement="room" />
        </div>
      )}
      {right && (
        <div className="absolute right-0 top-0 z-10 flex h-9" data-no-tabbar-drawer-toggle="right">
          <DrawerTabBarToggle side="right" placement="room" />
        </div>
      )}
    </>
  )
}
