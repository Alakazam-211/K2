// When a Home chat message shows its hover row (React, Reply, More).
//
// - One hover zone: the whole message, with the row inside it (it overlaps
//   the bubble's top edge, so there's no gap to cross).
// - Leaving hides it after HOVER_ROW_HIDE_MS, so a pointer crossing an edge
//   (or brushing past the row's corner) never makes it blink; coming back in
//   time keeps it, in the same place.
// - Its place is measured once as it appears (`measure`), never on pointer
//   moves; until measured it stays hidden, so its first frame is in place.
// - One message's row at a time: entering another message hides the last
//   one at once rather than after the delay.
// - Keyboard focus inside the message shows it too, and `hold` (the picker
//   open) keeps it. A touch shows nothing: a long press opens the menu.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type FocusEvent, type PointerEvent } from 'react'

/** How long the row stays after the pointer leaves the message (ms). */
export const HOVER_ROW_HIDE_MS = 160

/** The message whose row shows from hover now: hidden at once when the
 *  pointer enters another one. */
let hoverOwner: (() => void) | null = null

/** A focus this soon after a press is the press's (ms). */
const PRESS_FOCUS_MS = 600
let lastPress = -Infinity
let pressWatch = false
function watchPresses(): void {
  if (pressWatch || typeof document === 'undefined') return
  pressWatch = true
  document.addEventListener('pointerdown', () => (lastPress = performance.now()), true)
}

/** Focus from the keyboard (Tab, or Esc back from the picker), not from a
 *  click or a click's pick: a click on a message shouldn't pin its row once
 *  the pointer has gone. Read from the last press rather than
 *  `:focus-visible`, which engines decide differently for focus moved by
 *  script. */
function keyboardFocus(): boolean {
  return performance.now() - lastPress > PRESS_FOCUS_MS
}

export interface HoverReveal<P> {
  shown: boolean
  place: P | null
  handlers: {
    onPointerEnter: (e: PointerEvent<HTMLElement>) => void
    onPointerLeave: (e: PointerEvent<HTMLElement>) => void
    onFocus: (e: FocusEvent<HTMLElement>) => void
    onBlur: (e: FocusEvent<HTMLElement>) => void
  }
}

export function useHoverReveal<P>(measure: () => P | null, hold: boolean, enabled = true): HoverReveal<P> {
  const [hover, setHover] = useState(false)
  const [focus, setFocus] = useState(false)
  const [place, setPlace] = useState<P | null>(null)
  const hoverRef = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const clear = (): void => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
  }
  const hideNow = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
    hoverRef.current = false
    setHover(false)
    if (hoverOwner === hideNow) hoverOwner = null
  }, [])
  useEffect(
    () => () => {
      if (timer.current !== null) clearTimeout(timer.current)
      if (hoverOwner === hideNow) hoverOwner = null
    },
    [hideNow],
  )
  useEffect(watchPresses, [])
  // Opened some other way (the right-click menu's React…): place it first.
  const placed = place !== null
  const remeasure = useCallback(() => {
    const p = measure()
    if (p !== null) setPlace(p)
  }, [measure])
  useLayoutEffect(() => {
    if (hold && !placed) remeasure()
  }, [hold, placed, remeasure])

  // Showing already (or fading after a leave): keep its place, no jump.
  const showing = hover || focus || hold
  const handlers: HoverReveal<P>['handlers'] = {
    onPointerEnter: (e) => {
      if (!enabled || e.pointerType === 'touch') return
      clear()
      if (hoverOwner && hoverOwner !== hideNow) hoverOwner()
      hoverOwner = hideNow
      if (hoverRef.current) return
      hoverRef.current = true
      if (!showing) remeasure()
      setHover(true)
    },
    onPointerLeave: (e) => {
      if (!enabled || e.pointerType === 'touch') return
      clear()
      timer.current = setTimeout(hideNow, HOVER_ROW_HIDE_MS)
    },
    onFocus: () => {
      if (!enabled || focus || !keyboardFocus()) return
      if (!showing) remeasure()
      setFocus(true)
    },
    onBlur: (e) => {
      const next = e.relatedTarget
      if (next instanceof Node && e.currentTarget.contains(next)) return
      setFocus(false)
    },
  }
  return { shown: enabled && (hover || focus || hold) && place !== null, place, handlers }
}
