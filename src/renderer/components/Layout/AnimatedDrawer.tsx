// A drawer column that slides open/closed (0.46.0 UI): the outer box
// animates its width over DRAWER_ANIM_MS while the inner box keeps the
// drawer's own width (`w-max`), so the content is clipped, never squashed,
// and the room beside it reflows smoothly.
//
// Only an `open` change animates, from whatever toggled it (grip, tab bar
// or top bar button, context menu). No animation on first mount (boot,
// restore, page or window switch remount) or under prefers-reduced-motion,
// and none while dragging the resize edge: at rest the outer box has no
// inline width or transition and simply wraps the drawer.
//
// The terminal's PTY resize is already debounced (100 ms trailing) in
// TerminalPane, so the animation's per-frame size changes end in a single
// PTY resize; the Browser pane's ResizeObserver pushes its final bounds
// when the width settles. While the drawer slides, the Browser page is
// frozen (`beginBrowserMotion`): a picture of the page moves with the DOM
// instead of the native webview lagging the slide, released when the slide
// ends or is interrupted.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { flushSync } from 'react-dom'
import { DRAWER_ANIM_MS, DRAWER_TRANSITION, drawerMotionOn } from './drawer-motion'
import { beginBrowserMotion } from '@/lib/browser-overlay'

export default function AnimatedDrawer({
  open,
  side,
  bordered = true,
  clipAtRest = true,
  children,
}: {
  open: boolean
  side: 'left' | 'right'
  /** Draw the border on the room side (off when the drawer draws its own). */
  bordered?: boolean
  /** Clip at rest too (off when the drawer's resize edge hangs past it). */
  clipAtRest?: boolean
  children: ReactNode
}): React.JSX.Element | null {
  const [mounted, setMounted] = useState(open)
  const outerRef = useRef<HTMLDivElement>(null)
  const innerRef = useRef<HTMLDivElement>(null)
  const firstRun = useRef(true)
  const pending = useRef<'open' | 'close' | null>(null)
  const freshMount = useRef(false)
  const stop = useRef<(() => void) | null>(null)

  const clearInline = useCallback(() => {
    stop.current?.()
    stop.current = null
    const outer = outerRef.current
    if (outer) {
      outer.style.transition = ''
      outer.style.width = ''
      outer.style.overflow = ''
      delete outer.dataset.drawerAnim
    }
  }, [])

  const animate = useCallback(() => {
    const dir = pending.current
    const outer = outerRef.current
    const inner = innerRef.current
    if (!dir || !outer || !inner) return
    pending.current = null
    stop.current?.()
    stop.current = null

    const from = freshMount.current ? 0 : outer.getBoundingClientRect().width
    freshMount.current = false
    const border = outer.offsetWidth - outer.clientWidth
    const to = dir === 'open' ? inner.offsetWidth + border : 0

    outer.style.transition = 'none'
    outer.style.width = `${from}px`
    void outer.offsetWidth // commit the start width before transitioning
    outer.style.overflow = 'hidden'
    outer.dataset.drawerAnim = dir === 'open' ? 'opening' : 'closing'
    outer.style.transition = DRAWER_TRANSITION
    outer.style.width = `${to}px`

    // Freeze the Browser page for the slide; cleanup() releases it whether
    // the slide finishes, is interrupted by another toggle, or unmounts.
    let endBrowserFreeze: (() => void) | null = beginBrowserMotion()
    let timer: ReturnType<typeof setTimeout> | null = null
    const finish = (): void => {
      cleanup()
      stop.current = null
      if (dir === 'close') {
        // Unmount in this same task, still at 0 width. Clearing the inline
        // width first and leaving the unmount to React's next render let
        // the browser paint one frame of the drawer at full width: the
        // "flicker open" at the end of every close.
        flushSync(() => setMounted(false))
        return
      }
      outer.style.transition = ''
      outer.style.width = ''
      outer.style.overflow = ''
      delete outer.dataset.drawerAnim
    }
    const onEnd = (e: TransitionEvent): void => {
      if (e.target === outer && e.propertyName === 'width') finish()
    }
    const cleanup = (): void => {
      outer.removeEventListener('transitionend', onEnd)
      if (timer) clearTimeout(timer)
      endBrowserFreeze?.()
      endBrowserFreeze = null
    }
    outer.addEventListener('transitionend', onEnd)
    // Fallback when transitionend never fires (hidden window, no layout).
    timer = setTimeout(finish, DRAWER_ANIM_MS + 80)
    stop.current = cleanup
  }, [])

  useLayoutEffect(() => {
    if (firstRun.current) {
      firstRun.current = false
      return
    }
    if (!drawerMotionOn()) {
      pending.current = null
      clearInline()
      setMounted(open)
      return
    }
    pending.current = open ? 'open' : 'close'
    if (open && !mounted) {
      freshMount.current = true
      setMounted(true) // the effect below animates once the box exists
      return
    }
    animate()
    // `mounted` is read, not tracked: only `open` changes start an animation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  useLayoutEffect(() => {
    if (mounted && pending.current === 'open') animate()
  }, [mounted, animate])

  useEffect(() => () => stop.current?.(), [])

  if (!mounted) return null
  return (
    <div
      ref={outerRef}
      data-drawer-side={side}
      className={`flex h-full min-h-0 flex-shrink-0 flex-col ${clipAtRest ? 'overflow-hidden' : ''} ${
        bordered ? `border-[var(--color-border)] ${side === 'left' ? 'border-r' : 'border-l'}` : ''
      }`}
    >
      <div ref={innerRef} className="flex h-full min-h-0 w-max flex-col">
        {children}
      </div>
    </div>
  )
}
