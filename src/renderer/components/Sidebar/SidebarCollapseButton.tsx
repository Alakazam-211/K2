import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useSidebarStore } from '@/stores/sidebar'
import { useSetupNotFinished } from '@/stores/first-run'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { Button } from '@/components/ui'

/** The footer's « slides in over this long, on the Style's motion tokens. */
export const NAV_COLLAPSE_SLIDE = 'width var(--motion-duration-slow) var(--motion-ease-standard)'

/** The footer row's gap before «, held inside «'s slot so the slot can
 *  close to nothing and the main action takes the whole row. */
const NAV_COLLAPSE_GAP_PX = 8

/** Past the slowest Style's `--motion-duration-slow` (320ms): when the slot
 *  stops clipping if no transitionend came. */
const NAV_COLLAPSE_SETTLE_MS = 400

/** Collapse or open the workspaces nav. Lives at the bottom of the nav
 *  (collapse) and on the rails (open). The collapse side is gone until
 *  setup is finished (`setupNotFinished`: every Get started step done or
 *  skipped), whether the list is open or hidden as its button; a nav
 *  already collapsed stays so, and the rail still opens it.
 *
 *  `footer` (the nav footer): the shared quiet Button, one system with
 *  Add Workspace / Add Agent and Get started (the control edge, 3:1, with
 *  cut corners in Terminal and round in Glass; not Frost, whose rim is
 *  under 3:1 on the nav). It sits in a slot that closes to nothing while
 *  setup isn't finished, so the main action takes the whole row; when setup
 *  finishes the slot opens and « slides in as the main action shrinks
 *  (instant under reduced motion). Closed, « is out of the tab order and
 *  the accessibility tree, and inert. */
export function SidebarCollapseButton({ footer = false }: { footer?: boolean } = {}): React.JSX.Element | null {
  const collapsed = useSidebarStore((s) => s.isCollapsed)
  const toggle = useSidebarStore((s) => s.toggle)
  const setupNotFinished = useSetupNotFinished()
  const hidden = setupNotFinished && !collapsed
  if (hidden && !footer) return null
  const label = collapsed ? 'Open workspaces sidebar' : 'Collapse workspaces sidebar'
  const glyph = <DoubleChevron direction={collapsed ? 'right' : 'left'} className="w-4 h-4" />
  if (footer) return <FooterCollapseSlot hidden={hidden} label={label} glyph={glyph} onToggle={toggle} />
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

function FooterCollapseSlot({
  hidden,
  label,
  glyph,
  onToggle,
}: {
  hidden: boolean
  label: string
  glyph: React.ReactNode
  onToggle: () => void
}): React.JSX.Element {
  const reduced = useReducedMotion()
  const btnRef = useRef<HTMLButtonElement>(null)
  // «'s own width (32px in Terminal, 30px in Glass), read from the button,
  // which stays drawn inside the closed slot so it can be measured.
  const [btnWidth, setBtnWidth] = useState<number | null>(null)
  useLayoutEffect(() => {
    const w = btnRef.current?.offsetWidth ?? 0
    if (w > 0 && w !== btnWidth) setBtnWidth(w)
  })
  const open = btnWidth === null ? undefined : btnWidth + NAV_COLLAPSE_GAP_PX
  // The slot clips « while it's closed or sliding in, then lets go, so the
  // focus ring (2px out in Glass) isn't cut off.
  const [settled, setSettled] = useState(!hidden)
  useEffect(() => {
    if (hidden) {
      setSettled(false)
      return
    }
    const t = setTimeout(() => setSettled(true), reduced ? 0 : NAV_COLLAPSE_SETTLE_MS)
    return () => clearTimeout(t)
  }, [hidden, reduced])
  return (
    <div
      data-nav-collapse-slot={hidden ? 'closed' : 'open'}
      className={'flex flex-shrink-0 justify-end' + (settled ? '' : ' overflow-hidden')}
      onTransitionEnd={(e) => {
        if (e.target === e.currentTarget && !hidden) setSettled(true)
      }}
      // Opening slides (the main action shrinks as « comes in); closing is
      // instant, so a list loading in never animates « away.
      style={{
        width: hidden ? 0 : open,
        paddingLeft: open === undefined && !hidden ? NAV_COLLAPSE_GAP_PX : undefined,
        transition: hidden || reduced ? 'none' : NAV_COLLAPSE_SLIDE,
      }}
    >
      <Button
        ref={btnRef}
        variant="quiet"
        size="md"
        data-frost-shape="round"
        data-nav-control=""
        data-nav-collapse=""
        className="flex items-center justify-center w-8 h-8 p-0! flex-shrink-0 [--k2-ch-bw:1px]"
        onClick={hidden ? undefined : () => onToggle()}
        title={hidden ? undefined : label}
        aria-label={label}
        aria-hidden={hidden ? true : undefined}
        tabIndex={hidden ? -1 : undefined}
        inert={hidden}
      >
        {glyph}
      </Button>
    </div>
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
