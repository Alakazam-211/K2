// The sidebar's floor, moved gracefully (K2 Design). While the setup list is
// open the sidebar is drawn at least SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH wide
// (`displayedSidebarWidth`). When the list opens out of its Get started
// button, the sidebar widens to the floor; when it folds away or setup
// finishes, it narrows back to the saved width. Both on the Style's slow
// motion tokens, the same as the list's fold, and starting in the same frame
// (the floor and the fold follow the same store change).
//
// Web Animations on the sidebar's box: React keeps the inline width at the
// target, the animation runs from the old width over it and simply ends; no
// fill, no inline styles to clean up, and the saved width is never touched.
//
// Never animates: at boot or right after a server switch (the list wasn't
// known yet), during a drag on the resize edge, when the floor doesn't change
// the drawn width (a saved width at or above it), or under reduced motion
// (instant).
//
// The room beside it reflows as the sidebar moves, as it does for the
// drawers (`AnimatedDrawer`): the terminal's PTY resize is debounced (100ms
// trailing) in TerminalPane, so the terminal re-flows its text once, after
// the motion; the Browser page is frozen to a picture for the motion
// (`beginBrowserMotion`) and pushes its final bounds when it ends.

import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { beginBrowserMotion } from '@/lib/browser-overlay'
import { slowMotionTiming } from '@/lib/motion-timing'
import { useFirstRunKnown, useSetupChecklistShowing } from '@/stores/first-run'
import { isSidebarResizing } from '@/stores/sidebar'

export function useSidebarFloorMotion(ref: RefObject<HTMLElement | null>, width: number): void {
  const showing = useSetupChecklistShowing()
  const known = useFirstRunKnown()
  const reduced = useReducedMotion()
  const prev = useRef<{ width: number; showing: boolean; known: boolean } | null>(null)
  const anim = useRef<Animation | null>(null)
  const endFreeze = useRef<(() => void) | null>(null)

  const stop = (): void => {
    anim.current?.cancel()
    anim.current = null
    endFreeze.current?.()
    endFreeze.current = null
  }

  useLayoutEffect(() => {
    const was = prev.current
    prev.current = { width, showing, known }
    const el = ref.current
    if (!was || !el) return
    const floorMoved = was.showing !== showing && was.known && known && was.width !== width
    if (!floorMoved) {
      // Anything else that changes the width (a drag) wins over a motion.
      if (anim.current && was.width !== width) stop()
      return
    }
    if (reduced || isSidebarResizing() || typeof el.animate !== 'function') {
      stop()
      return
    }
    // From where it is: mid-motion, the width on screen.
    const from = anim.current ? el.getBoundingClientRect().width : was.width
    stop()
    const { duration, easing } = slowMotionTiming()
    let a: Animation
    try {
      a = el.animate([{ width: `${from}px` }, { width: `${width}px` }], { duration, easing })
    } catch {
      return
    }
    anim.current = a
    endFreeze.current = beginBrowserMotion()
    const done = (): void => {
      if (anim.current !== a) return
      anim.current = null
      endFreeze.current?.()
      endFreeze.current = null
    }
    a.finished.then(done, done)
  }, [width, showing, known, reduced]) // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => stop, [])
}
