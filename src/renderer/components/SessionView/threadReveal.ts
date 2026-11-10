// Revealing the terminal in Thread view (prd-thread-view-focus-v1 §6).
//
// The terminal stays display:none until revealed (a hidden but measurable
// box would send the daemon resizes: the old "zoom on workspace switch"
// bug). It then slides out from the left, or down from the top when the
// pane is narrow, at its final size: the layout is set once and only
// `transform` moves, so TerminalPane's ResizeObserver fires once and the PTY
// gets one resize through the existing debounce. Measured on the pane, not
// the window.

import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { drawerMotionOn } from '@/components/Layout/drawer-motion'
import { focusThreadComposeSlotTextarea } from '@/lib/workspace-switch-focus'
import { threadFloatsIn } from './threadFloats'
import type { ThreadDrawerKind } from './threadPanels'

export type RevealPlacement = 'left' | 'top'

/** The terminal's 420px minimum plus 480px of text (where the Thread already
 *  treats itself as narrow). */
export const REVEAL_LEFT_MIN_PX = 900
/** Back to the left only at this width, so it can't flip at the edge. */
export const REVEAL_LEFT_RETURN_PX = 924

export const REVEAL_LEFT_FRACTION = 0.45
export const REVEAL_LEFT_MIN_SIZE_PX = 420
export const REVEAL_LEFT_MAX_SIZE_PX = 640

export const REVEAL_TOP_FRACTION = 0.45
export const REVEAL_TOP_MIN_SIZE_PX = 180
/** Left for the Thread and its message box, where the pane allows. */
export const REVEAL_TOP_THREAD_PX = 240

/** The placement is re-checked this long after the pane's size stops changing. */
export const REVEAL_PLACEMENT_SETTLE_MS = 150

/** Left or top for a pane this wide. `was` is the current placement
 *  (null when opening); a 0 width (not laid out) keeps it. */
export function revealPlacementNext(width: number, was: RevealPlacement | null): RevealPlacement {
  if (!(width > 0)) return was ?? 'left'
  if (was === 'top') return width >= REVEAL_LEFT_RETURN_PX ? 'left' : 'top'
  return width >= REVEAL_LEFT_MIN_PX ? 'left' : 'top'
}

/** The terminal's width (left) or height (top) for this pane, in px. */
export function revealTerminalSizePx(placement: RevealPlacement, paneW: number, paneH: number): number {
  if (placement === 'left') {
    const raw = paneW * REVEAL_LEFT_FRACTION
    return Math.round(Math.min(REVEAL_LEFT_MAX_SIZE_PX, Math.max(REVEAL_LEFT_MIN_SIZE_PX, raw)))
  }
  const raw = Math.min(paneH * REVEAL_TOP_FRACTION, paneH - REVEAL_TOP_THREAD_PX)
  return Math.round(Math.max(REVEAL_TOP_MIN_SIZE_PX, raw))
}

// ── Slide timing: the Style's own tokens ───────────────────────────────

/** `150ms` / `0.15s` → ms. Null for anything else. */
export function parseCssDurationMs(raw: string): number | null {
  const v = raw.trim()
  const m = /^(-?[\d.]+)(ms|s)$/.exec(v)
  if (!m) return null
  const n = parseFloat(m[1])
  if (!Number.isFinite(n) || n < 0) return null
  return m[2] === 's' ? n * 1000 : n
}

function slideTiming(el: Element): { durationMs: number; easing: string } {
  const style = getComputedStyle(el)
  const durationMs = parseCssDurationMs(style.getPropertyValue('--motion-duration-medium')) ?? 150
  const easing = style.getPropertyValue('--motion-ease-standard').trim() || 'linear'
  return { durationMs, easing }
}

// ── The layout and the motion ───────────────────────────────────────────

export interface TerminalReveal {
  /** What the column holds: the terminal, or one of Thread view's panels
   *  (`threadPanels.ts`), which use the same column, size and slide. While it
   *  slides shut, the drawer that was open. Absent: the terminal. */
  kind?: ThreadDrawerKind
  /** The person wants it revealed (any drawer is open). */
  open: boolean
  /** It is laid out: open, or still sliding shut. */
  shown: boolean
  /** The conversation this memory belongs to. Motion is only for a toggle:
   *  a change of conversation or a restore never animates. */
  motionKey: string | null
  /** Sliding shut has ended (or there is no slide): hide the column. */
  onClosed: () => void
}

/** Whether the revealed terminal is laid out: `open`, or sliding shut. Set
 *  during render, so the commit that closes it still has the column up.
 *  `onClosed` ends the slide; leaving Thread view ends it at once. */
export function useTerminalRevealPresence(
  open: boolean,
  inThreadView: boolean,
): { shown: boolean; onClosed: () => void } {
  const [prevOpen, setPrevOpen] = useState(open)
  const [closing, setClosing] = useState(false)
  if (prevOpen !== open) {
    setPrevOpen(open)
    setClosing(!open)
  }
  if (!inThreadView && closing) setClosing(false)
  const onClosed = useCallback(() => setClosing(false), [])
  return { shown: open || (inThreadView && closing), onClosed }
}

export interface TerminalRevealLayout {
  placement: RevealPlacement
  sizePx: number
}

function slide(
  terminal: HTMLElement,
  thread: HTMLElement,
  floats: HTMLElement | null,
  placement: RevealPlacement,
  sizePx: number,
): Animation[] {
  const { durationMs, easing } = slideTiming(terminal)
  const options: KeyframeAnimationOptions = { duration: durationMs, easing, fill: 'both' }
  const terminalOff = placement === 'left' ? 'translateX(-100%)' : 'translateY(-100%)'
  const anims = [terminal.animate([{ transform: terminalOff }, { transform: 'none' }], options)]
  if (placement === 'left') {
    // The Thread starts where it was: its centred column glides over.
    anims.push(thread.animate([{ transform: `translateX(${-sizePx / 2}px)` }, { transform: 'none' }], options))
  } else {
    // From the top the Thread doesn't move at all: it stays full height, under
    // the terminal, for the whole slide (constant keyframes, so no layout per
    // frame). Its box changes once, where the terminal hides the change, and
    // the message box and newest messages never move.
    const held = { marginTop: `${-sizePx}px` }
    anims.push(thread.animate([held, held], options))
  }
  // The floating objects ride the terminal's edge on the same clock, so they
  // are never over it, at any point in the slide.
  if (floats) {
    const edge = placement === 'left' ? 'left' : 'top'
    anims.push(floats.animate([{ [edge]: '0px' }, { [edge]: `${sizePx}px` }], options))
  }
  return anims
}

/** The Thread's box is about to change or just has, by `deltaPx` at its top:
 *  keep what's being read where it is. At the bottom of the list this does
 *  nothing (the list's pin wins). */
function keepReadingPlace(thread: HTMLElement, deltaPx: number): void {
  const list = thread.querySelector('[data-testid="thread-overlay-list"]')
  if (list) list.scrollTop += deltaPx
}

/** Hiding while focus is in the terminal moves it to the Thread's message box. */
function moveFocusOffTerminal(terminal: HTMLElement, thread: HTMLElement): void {
  if (!terminal.contains(document.activeElement)) return
  const slot = thread.querySelector('[data-testid="agent-session-thread-compose-slot"]')
  focusThreadComposeSlotTextarea(slot, null)
}

/** Measure, place and slide the revealed terminal. Null `reveal`: this pane
 *  isn't in Thread view, nothing here runs. */
export function useTerminalRevealLayout(
  reveal: TerminalReveal | null | undefined,
  refs: {
    container: RefObject<HTMLElement | null>
    terminal: RefObject<HTMLElement | null>
    thread: RefObject<HTMLElement | null>
  },
): TerminalRevealLayout {
  const shown = !!reveal?.shown
  const [box, setBox] = useState({ w: 0, h: 0 })
  const [placement, setPlacement] = useState<RevealPlacement>('left')
  const placementRef = useRef(placement)
  placementRef.current = placement
  const latest = useRef(reveal)
  latest.current = reveal

  const motion = useRef({
    available: !!reveal,
    open: !!reveal?.open,
    key: reveal?.motionKey ?? null,
    anims: [] as Animation[],
  })
  const clearSlides = useCallback(() => {
    for (const a of motion.current.anims) a.cancel()
    motion.current.anims = []
  }, [])

  // Measure once at the start, then follow the pane. A size change that
  // doesn't flip the placement just follows it; the flip waits until the
  // size has settled and doesn't animate.
  const { container: containerRef } = refs
  useLayoutEffect(() => {
    const el = containerRef.current
    if (!shown || !el) return
    const rect = el.getBoundingClientRect()
    const initial = revealPlacementNext(rect.width, null)
    placementRef.current = initial
    setPlacement(initial)
    setBox({ w: rect.width, h: rect.height })
    if (typeof ResizeObserver === 'undefined') return
    let timer: ReturnType<typeof setTimeout> | null = null
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect()
      setBox((prev) => (prev.w === r.width && prev.h === r.height ? prev : { w: r.width, h: r.height }))
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        const next = revealPlacementNext(el.getBoundingClientRect().width, placementRef.current)
        if (next === placementRef.current) return
        placementRef.current = next
        // Not a toggle: no slide. One sliding shut finishes now.
        if (motion.current.anims.length > 0) {
          clearSlides()
          if (!motion.current.open) latest.current?.onClosed()
        }
        setPlacement(next)
      }, REVEAL_PLACEMENT_SETTLE_MS)
    })
    ro.observe(el)
    return () => {
      if (timer) clearTimeout(timer)
      ro.disconnect()
    }
  }, [shown, containerRef, clearSlides])

  // Column hidden: nothing may stay held at the slide's end.
  useLayoutEffect(() => {
    if (!shown) clearSlides()
  }, [shown, clearSlides])

  const open = !!reveal?.open
  const available = !!reveal
  const motionKey = reveal?.motionKey ?? null
  const { terminal: terminalRef, thread: threadRef } = refs
  useLayoutEffect(() => {
    const m = motion.current
    const was = { available: m.available, open: m.open, key: m.key }
    m.available = available
    m.open = open
    m.key = motionKey
    if (was.available === available && was.open === open && was.key === motionKey) return
    const toggled = available && was.available && was.key === motionKey && was.open !== open
    const terminal = terminalRef.current
    const thread = threadRef.current
    const container = containerRef.current
    if (toggled && !open && terminal && thread) moveFocusOffTerminal(terminal, thread)
    if (
      !toggled ||
      !terminal ||
      !thread ||
      !container ||
      !drawerMotionOn() ||
      typeof terminal.animate !== 'function'
    ) {
      clearSlides()
      if (available && !open) latest.current?.onClosed()
      return
    }
    if (m.anims.length > 0) {
      // Toggled mid-slide: reverse from where it is.
      for (const a of m.anims) a.reverse()
      return
    }
    const rect = container.getBoundingClientRect()
    const place = open ? revealPlacementNext(rect.width, null) : placementRef.current
    const sizePx = revealTerminalSizePx(place, rect.width, rect.height)
    const anims = slide(terminal, thread, threadFloatsIn(container), place, sizePx)
    m.anims = anims
    if (!open) for (const a of anims) a.reverse()
    if (!open && place === 'top') keepReadingPlace(thread, -sizePx)
    anims[0].onfinish = () => {
      if (motion.current.open) {
        clearSlides()
        if (place === 'top') keepReadingPlace(thread, sizePx)
      } else {
        latest.current?.onClosed()
      }
    }
  }, [available, open, motionKey, terminalRef, threadRef, containerRef, clearSlides])

  return { placement, sizePx: revealTerminalSizePx(placement, box.w, box.h) }
}

// ── The chord: which pane Ctrl+` acts on ────────────────────────────────

/** A mounted Terminal toggle (Thread view only). */
export interface RevealTarget {
  /** The room whose shortcut listener may act on it. */
  room: unknown
  element: HTMLElement
  enabled: () => boolean
  toggle: () => void
  /** The pane focus must be in for the chord to pick this target first.
   *  Absent: the session header's parent (Thread view). An agent tile's
   *  view picker names its tile. */
  pane?: () => HTMLElement | null
}

const targets = new Set<RevealTarget>()

export function registerRevealTarget(target: RevealTarget): () => void {
  targets.add(target)
  return () => {
    targets.delete(target)
  }
}

function targetPane(t: RevealTarget): HTMLElement | null {
  if (t.pane) return t.pane()
  return t.element.closest('[data-session-header]')?.parentElement ?? null
}

function targetGroup(t: RevealTarget): number | null {
  const raw = t.element.closest('[data-tab-group-index]')?.getAttribute('data-tab-group-index')
  if (raw == null) return null
  const n = Number(raw)
  return Number.isFinite(n) ? n : null
}

/** Act on the pane holding focus, else the active column's pane. False when
 *  there is no Thread view to act on (Terminal view keeps the key). */
export function toggleRevealForFocus(room: unknown, activeGroupIndex: number): boolean {
  const candidates = Array.from(targets).filter(
    (t) => t.room === room && t.enabled() && t.element.getClientRects().length > 0,
  )
  const active = document.activeElement
  const hit =
    candidates.find((t) => (active ? targetPane(t)?.contains(active) : false)) ??
    candidates.find((t) => {
      const group = targetGroup(t)
      return group === null || group === activeGroupIndex
    })
  if (!hit) return false
  hit.toggle()
  return true
}

/** Tests only. */
export function revealTargetsForTests(): Set<RevealTarget> {
  return targets
}
