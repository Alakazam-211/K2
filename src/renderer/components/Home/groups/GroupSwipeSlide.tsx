// Home groups: what the sideways swipe looks like in the nav and the rail
// (Rosson: slide only, no reduced-motion variant, and no words telling
// the person what to do).
//   - `GroupSwipeStrip` wraps the agent list. While a swipe is under way
//     the group it heads to rides in beside the list, at its edge, as one
//     strip: that group's rows with its glyph and name at the leading edge
//     (the peek, part of the motion, not a message).
//   - The strip follows the fingers 1:1, then glides on: to the next group
//     (the switch happens as it lands) or back home. Past the first or last
//     group it moves a third as far and glides back.
// The motion comes from `groupSwipeLive` (hooks/useGroupSwipe), not React:
// the transform is set on the strip in a rAF, so the list never re-renders
// per wheel event. Transform only (the GPU's), with a CSS transition for
// the glide. The main pane never moves.

import { useLayoutEffect, useRef } from 'react'
import { groupSwipeLive, useGroupSwipeStore, type SwipeFrame } from '@/hooks/useGroupSwipe'
import { useHomesStore, type Home } from '@/stores/homes'
import { GroupGlyph } from './GroupGlyph'

interface StripMotion {
  width: number
  applied: SwipeFrame | null
}

function shift(share: number, width: number): string {
  return `translate3d(${(-share * width).toFixed(2)}px, 0, 0)`
}

/** Put the strip where `f` says. Idempotent per frame. */
export function paintStrip(el: HTMLElement, f: SwipeFrame, m: StripMotion): void {
  if (m.applied === f) return
  const fromRest = !m.applied || m.applied.kind === 'rest'
  m.applied = f
  if (f.kind === 'rest') {
    el.style.transition = ''
    el.style.transform = ''
    el.style.willChange = ''
    // The incoming rows were drawn from the top: the landed list starts there too.
    if (f.landed) {
      const scroller = el.querySelector<HTMLElement>('[data-scroll-edges-list], [data-group-swipe-scroller]')
      if (scroller) scroller.scrollTop = 0
    }
    return
  }
  if (fromRest || m.width <= 0) m.width = el.offsetWidth
  el.style.willChange = 'transform'
  if (f.kind === 'track') {
    el.style.transition = 'none'
    el.style.transform = shift(f.offset, m.width)
    return
  }
  // The glide starts exactly where the fingers left it.
  el.style.transition = 'none'
  el.style.transform = shift(f.from, m.width)
  void el.offsetWidth
  el.style.transition = `transform ${f.ms}ms ${f.easing}`
  el.style.transform = shift(f.to, m.width)
  // Landing on the group: its name rides in and fades as the header takes it.
  const peek = el.querySelector<HTMLElement>('[data-group-swipe-peek]')
  if (peek && f.outcome === 'commit') {
    peek.style.transition = `opacity ${f.ms}ms ease-out`
    peek.style.opacity = '0'
  }
}

function useStripMotion(ref: React.RefObject<HTMLDivElement | null>, groupId: string): void {
  const motion = useRef<StripMotion>({ width: 0, applied: null })
  useLayoutEffect(() => {
    let raf = 0
    const apply = (): void => {
      raf = 0
      if (ref.current) paintStrip(ref.current, groupSwipeLive.get(), motion.current)
    }
    const off = groupSwipeLive.subscribe(() => {
      if (!raf) raf = requestAnimationFrame(apply)
    })
    apply()
    return () => {
      off()
      if (raf) cancelAnimationFrame(raf)
    }
  }, [ref])
  // A commit has landed and the new group's rows are drawn: the strip
  // comes back to rest in the same paint.
  useLayoutEffect(() => {
    if (ref.current && groupSwipeLive.get().kind === 'rest') paintStrip(ref.current, groupSwipeLive.get(), motion.current)
  }, [ref, groupId])
}

/** The incoming group's glyph and name at the leading edge of its rows
 *  (the glyph alone in the rail). A card: cut in Terminal like the top
 *  bar's floats (the surface chamfer; its shadow on the wrapper, since a
 *  box-shadow can't follow a cut), rounded in Glass. */
function GroupSwipePeek({ variant, group, index, sign }: { variant: 'nav' | 'rail'; group: Home; index: number; sign: number }): React.JSX.Element {
  const edge = sign > 0 ? 'left' : 'right'
  return (
    <div
      data-group-swipe-peek={group.id}
      data-k2-chamfer-float=""
      className="pointer-events-none absolute top-[38%] z-20 flex min-w-0 max-w-[calc(100%-16px)]"
      style={{ [edge]: variant === 'rail' ? 2 : 8 }}
    >
      <div data-group-float="peek" data-k2-chamfer="surface" className={`flex min-w-0 items-center gap-2 ${variant === 'rail' ? 'p-1.5' : 'px-3 py-2'}`}>
        <span className="flex-shrink-0 text-[var(--color-text-secondary)]">
          <GroupGlyph icon={group.icon ?? null} index={index} />
        </span>
        {variant === 'nav' && (
          <span className="truncate text-[13px] font-semibold text-[var(--color-text-primary)]">{group.name}</span>
        )}
      </div>
    </div>
  )
}

export interface GroupSwipeStripProps {
  variant: 'nav' | 'rail'
  /** The incoming group's rows, drawn as the list draws them (inert). */
  renderIncoming(group: Home): React.ReactNode
  /** The list itself. */
  children: React.ReactNode
}

/** The agent list and, during a swipe, the group it heads to beside it. */
export function GroupSwipeStrip({ variant, renderIncoming, children }: GroupSwipeStripProps): React.JSX.Element {
  // Only the direction re-renders this (the motion is groupSwipeLive's).
  const direction = useGroupSwipeStore((s) => s.swipe?.direction ?? null)
  const homes = useHomesStore((s) => s.homes)
  const selectedId = useHomesStore((s) => s.selectedId)
  const ref = useRef<HTMLDivElement>(null)
  useStripMotion(ref, selectedId)
  const at = homes.findIndex((h) => h.id === selectedId)
  const sign = direction === 'next' ? 1 : direction === 'prev' ? -1 : 0
  const to = at + sign
  const incoming = sign !== 0 && at >= 0 && to >= 0 && to < homes.length ? homes[to] : null
  return (
    <div ref={ref} className="relative flex min-h-0 w-full flex-1 flex-col" data-group-swipe-strip={variant}>
      {children}
      {incoming && (
        <div
          key={incoming.id}
          data-group-swipe-incoming={incoming.id}
          aria-hidden="true"
          inert
          className="pointer-events-none absolute inset-y-0 flex w-full flex-col"
          style={{ left: `${sign * 100}%` }}
        >
          {renderIncoming(incoming)}
          <GroupSwipePeek variant={variant} group={incoming} index={to} sign={sign} />
        </div>
      )}
    </div>
  )
}
