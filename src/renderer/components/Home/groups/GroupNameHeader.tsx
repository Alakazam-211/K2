// Home groups: the Home nav's top row. The current group's name and its
// agent count, then "…" (the Groups dialog on this group) and « (collapse
// the nav). The row is the shared tab-strip height (`TAB_STRIP_HEIGHT`), so
// its edges line up with the room's tab bar and the drawers' strips, and
// both buttons are the drawers' header icon buttons (`DrawerHeaderButton`),
// edge to edge as in a drawer strip. The name truncates before the buttons
// ever move. Plain text, not a menu: switching is the bar, the swipe and
// ⌥⌘1–9. Presentational apart from « (`useHomeGroups` wires the rest).

import { EllipsisIcon } from 'lucide-react'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { DrawerHeaderButton } from '@/components/Layout/DrawerTabBarToggle'
import { DoubleChevron } from '@/components/Sidebar/SidebarCollapseButton'
import { TAB_STRIP_HEIGHT } from '@/lib/tab-strip'
import { useSidebarStore } from '@/stores/sidebar'
import { useSetupNotFinished } from '@/stores/first-run'

export interface GroupNameHeaderProps {
  name: string
  icon: string | null
  /** The group's 0-based position. */
  index: number
  count: number
  /** Open the Groups dialog on this group. */
  onEdit(): void
}

/** « in the top row. Like the footer's « it was: gone until setup is
 *  finished (every Get started step done or skipped), when ⌘B can't
 *  collapse the nav either. */
function NavCollapseHeaderButton(): React.JSX.Element | null {
  const toggle = useSidebarStore((s) => s.toggle)
  const setupNotFinished = useSetupNotFinished()
  if (setupNotFinished) return null
  return (
    <DrawerHeaderButton label="Collapse workspaces sidebar" data-nav-collapse="" className="-ml-2" onClick={() => toggle()}>
      <DoubleChevron direction="left" className="h-4 w-4" />
    </DrawerHeaderButton>
  )
}

export default function GroupNameHeader({ name, count, onEdit }: GroupNameHeaderProps): React.JSX.Element {
  return (
    <div
      className={`flex ${TAB_STRIP_HEIGHT} w-full items-stretch gap-2 border-b border-[var(--color-border)] pl-3`}
      data-group-name-header=""
    >
      {/* The name takes the room; the count sits at its end, by "…". */}
      <span className="flex min-w-0 flex-1 items-center gap-2" data-picker-label="">
        <span className="min-w-0 flex-1 truncate type-heading text-[var(--color-text-primary)]" data-group-name="" data-picker-name="" title={name}>
          {name}
        </span>
        <span className="flex-shrink-0 type-hint tabular-nums text-[var(--color-text-muted)]" aria-label={`${count} agents`} data-group-count="" data-picker-count="">
          {count}
        </span>
      </span>
      <DrawerHeaderButton label={`Edit groups (${name})`} aria-haspopup="dialog" data-group-edit-open="" onClick={() => onEdit()}>
        <EllipsisIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" focusable="false" />
      </DrawerHeaderButton>
      <NavCollapseHeaderButton />
    </div>
  )
}
