import { useSidebarStore } from '@/stores/sidebar'
import { useSetupChecklistShowing } from '@/stores/first-run'
import { Button } from '@/components/ui'

/** Collapse or open the workspaces nav. Lives at the bottom of the nav
 *  (collapse) and on the rails (open). The collapse side is hidden while
 *  the setup checklist shows, since the nav holds it until the intro is
 *  done; a nav already collapsed stays so, and the rail still opens it.
 *
 *  `reserveSlot` (the nav footer): the shared quiet Button, one system with
 *  Add Workspace / Add Agent and Get started (the control edge, 3:1, with
 *  cut corners in Terminal and round in Glass; not Frost, whose rim is
 *  under 3:1 on the nav). While the checklist hides
 *  it, it keeps its slot invisible (out of the tab order and the
 *  accessibility tree), so the footer row keeps one shape. */
export function SidebarCollapseButton({ reserveSlot = false }: { reserveSlot?: boolean } = {}): React.JSX.Element | null {
  const collapsed = useSidebarStore((s) => s.isCollapsed)
  const toggle = useSidebarStore((s) => s.toggle)
  const checklistShowing = useSetupChecklistShowing()
  const hidden = checklistShowing && !collapsed
  if (hidden && !reserveSlot) return null
  const label = collapsed ? 'Open workspaces sidebar' : 'Collapse workspaces sidebar'
  const glyph = <DoubleChevron direction={collapsed ? 'right' : 'left'} className="w-4 h-4" />
  if (reserveSlot) {
    return (
      <Button
        variant="quiet"
        size="md"
        data-frost-shape="round"
        data-nav-control=""
        data-nav-collapse=""
        className={'flex items-center justify-center w-8 h-8 p-0! flex-shrink-0 [--k2-ch-bw:1px]' + (hidden ? ' invisible' : '')}
        onClick={hidden ? undefined : () => toggle()}
        title={hidden ? undefined : label}
        aria-label={label}
        aria-hidden={hidden ? true : undefined}
        tabIndex={hidden ? -1 : undefined}
      >
        {glyph}
      </Button>
    )
  }
  return (
    <button
      type="button"
      data-material="frost"
      data-frost-shape="round"
      data-nav-control=""
      className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 bg-white/[0.04] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] transition-colors"
      onClick={() => toggle()}
      title={label}
      aria-label={label}
    >
      {glyph}
    </button>
  )
}

/** The nav's double-arrow glyph (» / «). Shared with the drawer toggles in
 *  the room and drawer tab bars (`Layout/DrawerTabBarToggle.tsx`). */
export function DoubleChevron({
  direction,
  className,
}: {
  direction: 'left' | 'right'
  className?: string
}): React.JSX.Element {
  return (
    <svg
      className={className}
      viewBox="0 0 14 14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      data-double-chevron={direction}
    >
      {direction === 'right' ? (
        <>
          <path d="M4.5 3.5 L8 7 L4.5 10.5" />
          <path d="M8 3.5 L11.5 7 L8 10.5" />
        </>
      ) : (
        <>
          <path d="M9.5 3.5 L6 7 L9.5 10.5" />
          <path d="M6 3.5 L2.5 7 L6 10.5" />
        </>
      )}
    </svg>
  )
}
