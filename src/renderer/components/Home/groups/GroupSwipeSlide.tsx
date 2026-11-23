// Home groups: what the sideways swipe looks like in the nav and the rail
// (Rosson: slide only, no reduced-motion variant, and no words telling
// the person what to do).
//   - `GroupSwipeStrip` wraps the agent list. The groups either side of
//     the current one sit beside it, just out of view, as one strip: each
//     a cheap copy of that group's rows (avatar, name, pin; no status,
//     presence, badges or effects). Rosson: no tile naming the
//     group; the bar's colours already say where it goes.
//   - The strip follows the fingers 1:1, then glides on: to the next group
//     (the switch happens as it lands) or back home. Past the first or last
//     group it moves a third as far and glides back.
// The motion comes from `groupSwipeLive` (hooks/useGroupSwipe), not React:
// the transform is set on the strip in a rAF, so nothing re-renders while
// the fingers move. The neighbours are drawn ahead of the gesture (0.46.18:
// 0.46.17 mounted the live rows of the group it headed to on the first
// wheel event, a long task right as the fingers started). Transform only
// (the GPU's), with a CSS transition for the glide, and no layout reads
// while it moves: the strip's width is kept by a ResizeObserver. The main
// pane never moves.
//
// The landing: a commit switches the selected group as the glide ends,
// but the strip holds at the landed place until the list under it (which
// draws the group a moment later, `useDeferredValue`, so a long list never
// blocks a frame) shows that group; then it rests in the same paint.

import { memo, useLayoutEffect, useRef } from 'react'
import { groupSwipeLive, type SwipeFrame } from '@/hooks/useGroupSwipe'
import { useHomesStore, type Home } from '@/stores/homes'

interface StripMotion {
  width: number
  applied: SwipeFrame | null
  /** The group the list shows (the strip's `groupId`). */
  groupId: string
  /** The group the list showed when the last glide started. */
  glideFrom: string | null
}

/** A landed strip waits at most this long for the list to draw the group. */
export const STRIP_HOLD_MAX_MS = 400

function shift(share: number, width: number): string {
  return `translate3d(${(-share * width).toFixed(2)}px, 0, 0)`
}

/** Put the strip where `f` says. Idempotent per frame. A landed rest waits
 *  (returns false) while the list still shows the group it glided from. */
export function paintStrip(el: HTMLElement, f: SwipeFrame, m: StripMotion): boolean {
  if (m.applied === f) return true
  if (f.kind === 'rest' && f.landed && m.glideFrom !== null && m.glideFrom === m.groupId) return false
  const before = m.applied
  const fromRest = !before || before.kind === 'rest'
  m.applied = f
  if (f.kind === 'rest') {
    m.glideFrom = null
    el.style.transition = ''
    el.style.transform = ''
    el.style.willChange = ''
    // The incoming rows were drawn from the top: the landed list starts there too.
    if (f.landed) {
      const scroller = el.querySelector<HTMLElement>('[data-scroll-edges-list], [data-group-swipe-scroller]')
      if (scroller) scroller.scrollTop = 0
    }
    return true
  }
  if (m.width <= 0) m.width = el.offsetWidth
  if (fromRest) el.style.willChange = 'transform'
  if (f.kind === 'track') {
    el.style.transition = 'none'
    el.style.transform = shift(f.offset, m.width)
    return true
  }
  // The glide starts exactly where the fingers left it. When the last
  // painted frame already put it there, the transition starts from that;
  // otherwise set it first and flush the style (not layout) once.
  m.glideFrom = m.groupId
  const from = shift(f.from, m.width)
  if (el.style.transform !== from || el.style.transition !== 'none') {
    el.style.transition = 'none'
    el.style.transform = from
    void getComputedStyle(el).transform
  }
  el.style.transition = `transform ${f.ms}ms ${f.easing}`
  el.style.transform = shift(f.to, m.width)
  return true
}

function useStripMotion(ref: React.RefObject<HTMLDivElement | null>, groupId: string): void {
  const motion = useRef<StripMotion>({ width: 0, applied: null, groupId, glideFrom: null })
  motion.current.groupId = groupId
  useLayoutEffect(() => {
    const el = ref.current
    let raf = 0
    let hold: ReturnType<typeof setTimeout> | null = null
    let held: SwipeFrame | null = null
    const apply = (): void => {
      raf = 0
      const f = groupSwipeLive.get()
      if (!ref.current || paintStrip(ref.current, f, motion.current) || held === f) return
      // Held for the list: never longer than STRIP_HOLD_MAX_MS (a group
      // picked back before the list drew it never changes `groupId`).
      held = f
      if (hold) clearTimeout(hold)
      hold = setTimeout(() => {
        hold = null
        if (groupSwipeLive.get() !== f || motion.current.applied === f) return
        motion.current.glideFrom = null
        if (ref.current) paintStrip(ref.current, f, motion.current)
      }, STRIP_HOLD_MAX_MS)
    }
    const off = groupSwipeLive.subscribe(() => {
      if (!raf) raf = requestAnimationFrame(apply)
    })
    // The width, kept outside the gesture (no layout read while it moves).
    const ro =
      el && typeof ResizeObserver !== 'undefined'
        ? new ResizeObserver((entries) => {
            const w = entries[entries.length - 1]?.contentRect.width ?? 0
            if (w > 0) motion.current.width = w
          })
        : null
    if (el) ro?.observe(el)
    apply()
    return () => {
      off()
      ro?.disconnect()
      if (hold) clearTimeout(hold)
      if (raf) cancelAnimationFrame(raf)
    }
  }, [ref])
  // The list now shows the landed group: the strip comes back to rest in
  // the same paint.
  useLayoutEffect(() => {
    if (ref.current && groupSwipeLive.get().kind === 'rest') paintStrip(ref.current, groupSwipeLive.get(), motion.current)
  }, [ref, groupId])
}

export interface GroupSwipeStripProps {
  variant: 'nav' | 'rail'
  /** The group the list (`children`) shows now. */
  groupId: string
  /** A neighbouring group's cheap rows (inert, drawn ahead of a swipe). */
  renderNeighbour(group: Home): React.ReactNode
  /** The list itself. */
  children: React.ReactNode
}

/** One neighbour, just out of view on its side. Redrawn only when that
 *  group changes. */
const Neighbour = memo(function Neighbour({
  group,
  side,
  render,
}: {
  group: Home
  side: -1 | 1
  render: (group: Home) => React.ReactNode
}): React.JSX.Element {
  return (
    <div
      data-group-swipe-incoming={group.id}
      data-group-swipe-side={side > 0 ? 'next' : 'prev'}
      aria-hidden="true"
      inert
      className="pointer-events-none absolute inset-y-0 flex w-full flex-col"
      style={{ left: `${side * 100}%` }}
    >
      {render(group)}
    </div>
  )
}, (a, b) => a.group === b.group && a.side === b.side)

/** The agent list, with the groups either side of it beside it. */
export function GroupSwipeStrip({ variant, groupId, renderNeighbour, children }: GroupSwipeStripProps): React.JSX.Element {
  const homes = useHomesStore((s) => s.homes)
  const ref = useRef<HTMLDivElement>(null)
  useStripMotion(ref, groupId)
  const at = homes.findIndex((h) => h.id === groupId)
  const prev = at > 0 ? homes[at - 1] : null
  const next = at >= 0 && at < homes.length - 1 ? homes[at + 1] : null
  return (
    <div ref={ref} className="relative flex min-h-0 w-full flex-1 flex-col" data-group-swipe-strip={variant}>
      {children}
      {prev && <Neighbour key={prev.id} group={prev} side={-1} render={renderNeighbour} />}
      {next && <Neighbour key={next.id} group={next} side={1} render={renderNeighbour} />}
    </div>
  )
}
