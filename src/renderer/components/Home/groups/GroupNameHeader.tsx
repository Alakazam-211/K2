// Home groups: the Home nav's top row. The current group's name with its
// agent count as a small tag right after it, then "…" (the Groups dialog on
// this group) and « (collapse the nav). The row is the shared tab-strip
// height (`TAB_STRIP_HEIGHT`), so its edges line up with the room's tab bar
// and the drawers' strips. Both buttons are the drawers' header icon buttons
// (`DrawerHeaderButton`) in their 'glyph' shape, edge to edge: the full row
// height to click, but hover and focus draw one small shape round the glyph
// (a circle in Glass, a chamfered square in Terminal; globals.css). The name
// truncates before the tag shrinks or the buttons move. Plain text, not a
// menu: switching is the bar, the swipe and ⌥⌘1–9. Presentational apart
// from « (`useHomeGroups` wires the rest).

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
    <DrawerHeaderButton label="Collapse workspaces sidebar" shape="glyph" data-nav-collapse="" className="-ml-2" onClick={() => toggle()}>
      <DoubleChevron direction="left" className="h-4 w-4" />
    </DrawerHeaderButton>
  )
}

/** The count tag: the nav's count-tag look (the Agents nav's `NavCountTag`,
 *  a plain number) on the theme's tokens, so it reads in light schemes too:
 *  the quiet wash (`--color-wash-2`) under secondary text, a pill in Glass
 *  and a chip cut in Terminal (`data-k2-chamfer="chip"`; the transparent
 *  border keeps the cut's diagonal edge invisible). */
export const GROUP_COUNT_TAG_CLASS =
  'flex-shrink-0 rounded-[var(--radius-pill)] border border-transparent bg-[var(--color-wash-2)] px-1.5 type-hint font-medium leading-4 tabular-nums text-[var(--color-text-secondary)]'

export default function GroupNameHeader({ name, count, onEdit }: GroupNameHeaderProps): React.JSX.Element {
  return (
    <div
      className={`flex ${TAB_STRIP_HEIGHT} w-full items-stretch gap-2 border-b border-[var(--color-border)] pl-3`}
      data-group-name-header=""
    >
      {/* The label takes the room; the count tag sits right after the name
          and never shrinks, so a long name truncates first. */}
      <span className="flex min-w-0 flex-1 items-center gap-1.5" data-picker-label="">
        <span className="min-w-0 truncate type-heading text-[var(--color-text-primary)]" data-group-name="" data-picker-name="" title={name}>
          {name}
        </span>
        <span className={GROUP_COUNT_TAG_CLASS} data-k2-chamfer="chip" aria-label={`${count} agents`} data-group-count="" data-picker-count="">
          {count}
        </span>
      </span>
      <DrawerHeaderButton label={`Edit groups (${name})`} shape="glyph" aria-haspopup="dialog" data-group-edit-open="" onClick={() => onEdit()}>
        <EllipsisIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" focusable="false" />
      </DrawerHeaderButton>
      <NavCollapseHeaderButton />
    </div>
  )
}
