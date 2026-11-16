// The live views' side panel opens and closes like the app's other drawers
// (Layout/AnimatedDrawer and drawer-motion): one piece, so the scene, the
// timeline and the editors all slide the same way.
//
//   - Beside the hero (wide, and the editor's reflow at 800): AnimatedDrawer
//     itself. Its box grows from 0 to the panel's width over DRAWER_ANIM_MS
//     on the drawers' ease while the panel keeps its own width inside it, so
//     the panel's rows never squash or re-wrap and the hero beside it reflows
//     in one smooth step. In Glass the sliding box is the drawers' tile (fill,
//     rim, rounded corners, the fade at the ends; styles/glass/overrides.css
//     `[data-drawer-side]`), floating in the tab with the page gap around it;
//     in Terminal it stays square and flush, with its hairline.
//   - Over the hero (the scene's and the timeline's sheet when narrow): the
//     sheet slides in from the right edge with a short fade, on the same
//     duration and ease, and out the same way.
//   - Reduced motion: it appears and goes at once.
//   - Closing with the keyboard inside the panel puts focus back on the
//     header's panel toggle.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import AnimatedDrawer from '@/components/Layout/AnimatedDrawer'
import { DRAWER_ANIM_MS, drawerMotionOn } from '@/components/Layout/drawer-motion'

/** Focus inside `box` goes back to the tab's panel toggle once the panel closes. */
function useFocusBackOnClose(open: boolean, box: React.RefObject<HTMLElement | null>): void {
  const was = useRef(open)
  useLayoutEffect(() => {
    const closed = was.current && !open
    was.current = open
    const el = box.current
    if (!closed || !el || !el.contains(document.activeElement)) return
    const shell = el.closest('.lv-shell') ?? document
    shell.querySelector<HTMLElement>('[data-testid="lv-panel-toggle"]')?.focus()
  }, [open, box])
}

/** The sheet over the hero: mounted while it slides out, then gone. */
function SheetSlide({ open, children }: { open: boolean; children: React.ReactNode }): React.JSX.Element | null {
  const [shown, setShown] = useState(open)
  const [phase, setPhase] = useState<'in' | 'out' | null>(null)
  const first = useRef(true)
  useLayoutEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    if (!drawerMotionOn()) {
      setPhase(null)
      setShown(open)
      return
    }
    if (open) {
      setShown(true)
      setPhase('in')
      return
    }
    setPhase('out')
    const t = setTimeout(() => {
      setShown(false)
      setPhase(null)
    }, DRAWER_ANIM_MS)
    return () => clearTimeout(t)
  }, [open])
  if (!shown) return null
  return (
    <div data-lv-sheet-anim={phase ?? undefined} className="lv-sheet-slide contents">
      {children}
    </div>
  )
}

export function SlidingPanel({ open, sheet, children }: { open: boolean; sheet: boolean; children: React.ReactNode }): React.JSX.Element {
  const box = useRef<HTMLDivElement>(null)
  useFocusBackOnClose(open, box)
  // The panel's box: `contents`, so the panel sits in the frame's row as before.
  return (
    <div ref={box} className="contents" data-lv-panel-motion={sheet ? 'sheet' : 'drawer'}>
      {sheet ? (
        <SheetSlide open={open}>{children}</SheetSlide>
      ) : (
        <AnimatedDrawer open={open} side="right" bordered={false}>
          {children}
        </AnimatedDrawer>
      )}
    </div>
  )
}

/** The motion's own styles (in the shell's sheet, scoped under `.lv-shell`). */
export const PANEL_MOTION_CSS = `
.lv-shell [data-lv-panel-motion="drawer"] > [data-drawer-side] { --k2-drawer-hide-start: 0px; height: auto; align-self: stretch; }
.lv-shell [data-lv-panel-motion="drawer"] .lv-panel { flex: 1 1 auto; min-height: 0; }
html[data-style="glass"] .lv-shell [data-lv-panel-motion="drawer"] > [data-drawer-side] { margin: var(--gap-pane) var(--gap-pane) var(--gap-pane) 0; }
html[data-style="glass"] .lv-shell [data-lv-panel-motion] .lv-panel { border-left: 0; }
html[data-style="glass"] .lv-shell [data-lv-panel-motion="drawer"] .lv-panel { background: transparent; }
html[data-style="glass"] .lv-shell [data-lv-panel-motion="sheet"] .lv-panel { margin: var(--gap-pane); border-radius: var(--radius-box); background: var(--tile-fill); border: 1px solid var(--tile-edge); box-shadow: var(--tile-sheen), var(--tile-shadow); }
.lv-shell [data-lv-sheet-anim="in"] > .lv-panel { animation: lv-sheet-in ${DRAWER_ANIM_MS}ms cubic-bezier(0.2, 0, 0, 1) both; }
.lv-shell [data-lv-sheet-anim="out"] > .lv-panel { animation: lv-sheet-out ${DRAWER_ANIM_MS}ms cubic-bezier(0.2, 0, 0, 1) both; }
@keyframes lv-sheet-in { from { opacity: 0; transform: translateX(24px); } }
@keyframes lv-sheet-out { to { opacity: 0; transform: translateX(24px); } }
`
