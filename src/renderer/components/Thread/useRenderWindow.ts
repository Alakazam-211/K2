// The chat render window (threadWindow.ts) as a hook: the desktop Thread,
// a ticket's discussion and the project chat. Ported from K2 Companion's
// `useRenderWindow.ts` (phone-perf 5f6c158), opening at the newest (the
// desktop chats all open at their end).
//
// Sizes (Rosson, 2026-10-18, as on the phone): a chat opens with
// WINDOW_ROWS (30) drawn; reading back, rows are drawn WINDOW_STEP (20) at
// a time before the reader reaches a drawn edge, up to WINDOW_MAX_ROWS (50).
// Past that, each step drops as many rows at the far end (the end the
// reader is reading away from), a spacer of their measured height standing
// in; reading forward draws them again and drops from the other end. Only
// the edge the reader is scrolling toward grows (a window at the cap would
// otherwise see-saw), and a row still on screen never drops. Leaving the
// chat and coming back (a new `room`, or a remount) starts at 30 again.
//
// Following the newest: the window is the newest 30; arrivals draw at once
// and the oldest drop off once scrolling has been quiet (WINDOW_IDLE_MS).
// An older page (the chat's own "Earlier messages" fetch) draws at once
// when the window already reaches the first row in memory; otherwise
// `growTop()` brings rows back from memory first.
//
// Rows are the list's direct children with `data-zen-message` (the
// Thread's bubble root); its value is the row's key. A window change holds
// the first row on screen where it was (scrollTop moves by its layout
// offset change). The list's scroller should set `overflow-anchor: none`.

import { useCallback, useEffect, useLayoutEffect, useReducer, useRef } from 'react'
import {
  measureRows,
  pinWindow,
  resolveWindow,
  spacerAbove,
  spacerBelow,
  WINDOW_IDLE_MS,
  WINDOW_KEEP_SCREENS,
  WINDOW_MAX_ROWS,
  WINDOW_RESPAWN_SCREENS,
  WINDOW_ROWS,
  WINDOW_STEP,
  type ChatWindow,
  type WindowRange,
} from './threadWindow'

/** The rows the window counts: the list's direct children with a key. */
export const WINDOW_ROW_SELECTOR = ':scope > [data-zen-message]'
const ROW_KEY_ATTR = 'data-zen-message'

export interface RenderWindowOptions {
  /** The chat's identity (the Thread's address, the ticket): a new one
   *  starts a fresh window. */
  room: string
  /** Every row's key in memory, oldest first. */
  keys: readonly string[]
  /** The scrolling element. */
  scroller: () => HTMLElement | null
  /** The element holding the rows (their parent). */
  list: () => HTMLElement | null
  /** The reader follows the newest (the list is pinned to its bottom). */
  following: () => boolean
  /** Pin the view to the newest (after the window moved back there). */
  stick: (el: HTMLElement) => void
  /** Something else is moving the list: wait. */
  busy?: () => boolean
  /** The window wrote the scroller's scrollTop (holding a row still). */
  onScrollWrite?: (top: number) => void
}

export interface RenderWindow extends WindowRange {
  /** Height (px) the rows above / below the window stand in for. */
  above: number
  below: number
  /** The newest / the oldest row in memory is drawn. */
  drawsNewest: boolean
  drawsOldest: boolean
  /** The row was drawn before and left (it comes back without the
   *  message-in animation). */
  wasDrawn: (key: string) => boolean
  /** Draw through the newest; `stick`: and land the view there. */
  toNewest: (stick: boolean) => void
  /** Memory first: draw a step of the rows above the window. False when
   *  the window already starts at the first row in memory. */
  growTop: () => boolean
  /** The scroller scrolled (call from its scroll handler). */
  onScroll: () => void
}

export function useRenderWindow(o: RenderWindowOptions): RenderWindow {
  const opts = useRef(o)
  opts.current = o
  const winRef = useRef<{ room: string; win: ChatWindow } | null>(null)
  const heights = useRef<{ room: string; px: Map<string, number> }>({ room: o.room, px: new Map() })
  const drawn = useRef<(WindowRange & { keys: readonly string[] }) | null>(null)
  const anchor = useRef<{ el: HTMLElement; top: number } | null>(null)
  const stickNext = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const raf = useRef(0)
  /** Which way the reader last scrolled: -1 up, 1 down, 0 not yet. */
  const dir = useRef(0)
  const lastTop = useRef(0)
  const lastScrollAt = useRef(0)
  const [, bump] = useReducer((n: number) => n + 1, 0)

  const { room, keys } = o
  if (heights.current.room !== room) heights.current = { room, px: new Map() }
  if (winRef.current && winRef.current.room !== room) winRef.current = null
  const range = resolveWindow(keys, winRef.current?.win ?? null)
  const above = spacerAbove(keys, range.lo, heights.current.px)
  const below = spacerBelow(keys, range.hi, heights.current.px)

  function rows(): HTMLElement[] {
    const list = opts.current.list()
    return list ? Array.from(list.querySelectorAll<HTMLElement>(WINDOW_ROW_SELECTOR)) : []
  }

  /** The first drawn row still (partly) on screen. */
  function captureAnchor(): { el: HTMLElement; top: number } | null {
    const el = opts.current.scroller()
    if (!el) return null
    const top = el.getBoundingClientRect().top
    for (const r of rows()) {
      if (r.getBoundingClientRect().bottom > top + 1) return { el: r, top: r.offsetTop }
    }
    return null
  }

  function rememberHeights(leaving: HTMLElement[]): void {
    const px = measureRows(leaving)
    leaving.forEach((r, i) => {
      const key = r.getAttribute(ROW_KEY_ATTR)
      if (key) heights.current.px.set(key, px[i])
    })
  }

  /** Draw another window. `hold`: keep the row under the reader's eye
   *  where it is. */
  function setWindow(win: ChatWindow, hold: boolean): void {
    winRef.current = { room: opts.current.room, win }
    anchor.current = hold ? captureAnchor() : null
    bump()
  }

  /** Grow one edge by a step; at the cap, drop rows wholly off screen at
   *  the other end (from the far end inward, never one on screen). */
  function grow(edge: 'top' | 'bottom', view: WindowRange & { keys: readonly string[] }, shown: HTMLElement[]): boolean {
    const { keys: k, lo, hi } = view
    const el = opts.current.scroller()
    const st = el?.scrollTop ?? 0
    const ch = el?.clientHeight ?? 0
    const scTop = el ? el.getBoundingClientRect().top : 0
    const y = (r: HTMLElement): number => r.getBoundingClientRect().top - scTop + st
    const indexOf = (r: HTMLElement): number => k.indexOf(r.getAttribute(ROW_KEY_ATTR) ?? '')
    const leaving: HTMLElement[] = []
    if (edge === 'top') {
      if (lo <= 0) return false
      const nextLo = Math.max(0, lo - WINDOW_STEP)
      let nextHi = hi
      for (let i = shown.length - 1; i > 0 && nextHi - nextLo > WINDOW_MAX_ROWS; i--) {
        if (ch > 0 && y(shown[i]) < st + ch) break
        if (indexOf(shown[i]) !== nextHi - 1) break
        leaving.push(shown[i])
        nextHi--
      }
      rememberHeights(leaving)
      setWindow(pinWindow(k, { lo: nextLo, hi: nextHi }), true)
      return true
    }
    if (hi >= k.length) return false
    const nextHi = Math.min(k.length, hi + WINDOW_STEP)
    let nextLo = lo
    for (let i = 0; i < shown.length - 1 && nextHi - nextLo > WINDOW_MAX_ROWS; i++) {
      if (ch > 0 && y(shown[i]) + shown[i].offsetHeight > st) break
      if (indexOf(shown[i]) !== nextLo) break
      leaving.push(shown[i])
      nextLo++
    }
    rememberHeights(leaving)
    setWindow(pinWindow(k, { lo: nextLo, hi: nextHi }), true)
    return true
  }

  function reconcile(idle: boolean): 'busy' | 'done' {
    const { scroller, following, busy } = opts.current
    const el = scroller()
    const view = drawn.current
    if (!el || !view || !view.keys.length) return 'done'
    if (busy?.()) return 'busy'
    const { keys: k, lo, hi } = view
    const len = k.length
    const shown = rows()
    if (!shown.length) return 'done'
    const indexOf = (r: HTMLElement): number => k.indexOf(r.getAttribute(ROW_KEY_ATTR) ?? '')
    if (following()) {
      if (hi < len) {
        stickNext.current = true
        setWindow(pinWindow(k, { lo, hi: len }), false)
        return 'done'
      }
      if (hi - lo <= WINDOW_ROWS) return 'done'
      if (!idle) return 'busy'
      const start = Math.max(0, len - WINDOW_ROWS)
      rememberHeights(
        shown.filter((r) => {
          const at = indexOf(r)
          return at >= 0 && at < start
        }),
      )
      setWindow(pinWindow(k, { lo: start, hi: len }), true)
      return 'done'
    }
    const ch = el.clientHeight
    if (!(ch > 0)) return 'done'
    const st = el.scrollTop
    const scTop = el.getBoundingClientRect().top
    const y = (r: HTMLElement): number => r.getBoundingClientRect().top - scTop + st
    const first = shown[0]
    const last = shown[shown.length - 1]
    const up = dir.current <= 0
    const down = dir.current >= 0
    if (up && lo > 0 && y(first) > st - WINDOW_RESPAWN_SCREENS * ch) {
      grow('top', view, shown)
      return 'done'
    }
    if (down && hi < len && y(last) + last.offsetHeight < st + ch + WINDOW_RESPAWN_SCREENS * ch) {
      grow('bottom', view, shown)
      return 'done'
    }
    // Over the cap (an older page drew at once, arrivals while reading
    // back): once quiet, rows far past the view at the end the reader is
    // moving away from leave.
    if (hi - lo > WINDOW_MAX_ROWS) {
      if (!idle) return 'busy'
      const keep = WINDOW_KEEP_SCREENS * ch
      const leaving: HTMLElement[] = []
      let nextLo = lo
      let nextHi = hi
      if (up) {
        for (let i = shown.length - 1; i > 0 && nextHi - nextLo > WINDOW_MAX_ROWS; i--) {
          if (y(shown[i]) < st + ch + keep || indexOf(shown[i]) !== nextHi - 1) break
          leaving.push(shown[i])
          nextHi--
        }
      } else {
        for (let i = 0; i < shown.length - 1 && nextHi - nextLo > WINDOW_MAX_ROWS; i++) {
          if (y(shown[i]) + shown[i].offsetHeight > st - keep || indexOf(shown[i]) !== nextLo) break
          leaving.push(shown[i])
          nextLo++
        }
      }
      if (!leaving.length) return 'done'
      rememberHeights(leaving)
      setWindow(pinWindow(k, { lo: nextLo, hi: nextHi }), true)
    }
    return 'done'
  }
  const reconcileRef = useRef(reconcile)
  reconcileRef.current = reconcile

  /** Rows leave only once scrolling has been quiet a while; a pending
   *  check stays (frequent commits must not keep pushing it back). */
  const scheduleIdle = useRef((delay: number = WINDOW_IDLE_MS): void => {
    if (timer.current) return
    timer.current = setTimeout(() => {
      timer.current = null
      const quiet = Date.now() - lastScrollAt.current
      if (quiet < WINDOW_IDLE_MS) {
        scheduleIdle.current(WINDOW_IDLE_MS - quiet + 10)
        return
      }
      if (reconcileRef.current(true) === 'busy') scheduleIdle.current()
    }, delay)
  })

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
      timer.current = null
      if (raf.current) cancelAnimationFrame(raf.current)
      raf.current = 0
    },
    [],
  )

  // After every commit: what was drawn (pinned by key, so arrivals and a
  // refetch do not move it), the held row put back, the stick, the trim.
  useLayoutEffect(() => {
    const { keys: k, scroller, stick, following, onScrollWrite } = opts.current
    drawn.current = { keys: k, lo: range.lo, hi: range.hi }
    if (k.length) winRef.current = { room: opts.current.room, win: pinWindow(k, range) }
    const a = anchor.current
    anchor.current = null
    const el = scroller()
    if (a && el && a.el.isConnected) {
      const dy = a.el.offsetTop - a.top
      if (Math.abs(dy) >= 1) {
        el.scrollTop += dy
        onScrollWrite?.(el.scrollTop)
      }
    }
    if (stickNext.current) {
      stickNext.current = false
      if (el && following()) {
        stick(el)
        onScrollWrite?.(el.scrollTop)
      }
    }
    if (k.length > WINDOW_ROWS || range.lo > 0 || range.hi < k.length) scheduleIdle.current()
  })

  const toNewest = useCallback((stick: boolean): void => {
    const view = drawn.current
    if (view && view.hi >= view.keys.length) return
    // The rows leaving keep their height (a spacer, not a jump).
    rememberHeights(rows())
    stickNext.current = stick
    const k = view?.keys ?? opts.current.keys
    setWindow(pinWindow(k, { lo: Math.max(0, k.length - WINDOW_ROWS), hi: k.length }), false)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const growTop = useCallback((): boolean => {
    const view = drawn.current
    if (!view || view.lo <= 0) return false
    return grow('top', view, rows())
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const onScroll = useCallback((): void => {
    const el = opts.current.scroller()
    if (el) {
      const top = el.scrollTop
      if (top !== lastTop.current) dir.current = top < lastTop.current ? -1 : 1
      lastTop.current = top
    }
    lastScrollAt.current = Date.now()
    if (!raf.current) {
      raf.current = requestAnimationFrame(() => {
        raf.current = 0
        reconcileRef.current(false)
      })
    }
    scheduleIdle.current()
  }, [])

  const px = heights.current.px
  const wasDrawn = useCallback((key: string): boolean => px.has(key), [px])

  return {
    ...range,
    above,
    below,
    drawsNewest: range.hi >= keys.length,
    drawsOldest: range.lo <= 0,
    wasDrawn,
    toNewest,
    growTop,
    onScroll,
  }
}
