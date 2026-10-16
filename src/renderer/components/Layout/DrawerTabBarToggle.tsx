// Drawer toggles in the tab bars (0.46.0 UI experiment, Agents page; the
// switch is `drawer-handles.ts`). Uses the nav's » / « glyph.
//
// - Closed drawer: its own slot at the FAR LEFT (left drawer, ») of the
//   leftmost room column's tab bar, or the FAR RIGHT (right drawer, «) of
//   the rightmost column's, after split / remove column.
// - Open drawer: the button moves into that drawer's tab strip, at the end
//   nearest the room (the drawer's inner edge), pointing at the window
//   edge it collapses to (« in the left drawer, » in the right).
//
// Open state stays per-window chrome (`usePanelsStore`).

import { DoubleChevron } from '@/components/Sidebar/SidebarCollapseButton'
import { usePanelsStore } from '@/stores/panels'

type Side = 'left' | 'right'

export default function DrawerTabBarToggle({
  side,
  placement,
}: {
  side: Side
  /** 'room': in the room's tab bar (drawer closed). 'drawer': in the drawer's own tab strip (open). */
  placement: 'room' | 'drawer'
}): React.JSX.Element {
  const toggle = usePanelsStore((s) => (side === 'left' ? s.toggleLeftPanel : s.toggleRightPanel))
  const opening = placement === 'room'
  // Opening points toward the room (where the drawer slides in from the
  // edge); closing points at the edge the drawer collapses to.
  const direction: Side = opening ? (side === 'left' ? 'right' : 'left') : side
  const label = `${opening ? 'Show' : 'Hide'} ${side} drawer`
  return (
    <button
      type="button"
      onClick={() => toggle()}
      title={label}
      aria-label={label}
      aria-expanded={!opening}
      data-drawer-tabbar-toggle={side}
      data-placement={placement}
      className="no-drag flex h-full w-8 flex-shrink-0 items-center justify-center bg-white/[0.04] text-[var(--color-text-secondary)] outline-none transition-colors hover:bg-white/[0.08] hover:text-[var(--color-text-primary)] focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[var(--color-accent)]"
    >
      <DoubleChevron direction={direction} className="h-4 w-4" />
    </button>
  )
}
