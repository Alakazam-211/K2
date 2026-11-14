// Home groups: "Most recent at the top" moves an agent up when you message
// it. The move is gentle, not a jump (K2 Design's mock, "an agent moving
// up"): the rows glide from where they were to where they are on the slow
// motion token, and the row that climbed is lifted while it passes the
// others. Only within one group, only with the setting on; a group switch
// or a manual reorder just lands.

import { useLayoutEffect, useRef, type RefObject } from 'react'

/** For each key, how far (px) it moved since `before`: positive is down. */
export function rowShifts(before: ReadonlyMap<string, number>, after: ReadonlyMap<string, number>): Map<string, number> {
  const out = new Map<string, number>()
  for (const [k, y] of after) {
    const was = before.get(k)
    if (was !== undefined && was !== y) out.set(k, was - y)
  }
  return out
}

function positions(container: HTMLElement, selector: string, attr: string): Map<string, number> {
  const out = new Map<string, number>()
  for (const el of Array.from(container.querySelectorAll<HTMLElement>(selector))) {
    const k = el.getAttribute(attr)
    if (k) out.set(k, el.offsetTop)
  }
  return out
}

export function useRowFlip(
  container: RefObject<HTMLElement | null>,
  order: readonly string[],
  enabled: boolean,
  groupId: string,
  selector = '[data-home-row]',
  attr = 'data-home-row',
): void {
  const last = useRef<{ group: string; at: Map<string, number> } | null>(null)
  const key = order.join('\n')
  useLayoutEffect(() => {
    const box = container.current
    if (!box) return
    const now = positions(box, selector, attr)
    const prev = last.current
    last.current = { group: groupId, at: now }
    if (!enabled || !prev || prev.group !== groupId) return
    const shifts = rowShifts(prev.at, now)
    if (shifts.size === 0) return
    const climber = [...shifts.entries()].reduce((a, b) => (b[1] > a[1] ? b : a))[0]
    const els = Array.from(box.querySelectorAll<HTMLElement>(selector))
    const moving = els.filter((el) => shifts.has(el.getAttribute(attr) ?? ''))
    for (const el of moving) {
      el.style.transition = 'none'
      el.style.transform = `translateY(${shifts.get(el.getAttribute(attr) ?? '')}px)`
      if (el.getAttribute(attr) === climber) el.setAttribute('data-row-lift', '')
    }
    const raf = requestAnimationFrame(() => {
      for (const el of moving) {
        el.style.transition = 'transform var(--motion-duration-slow) var(--motion-ease-out-soft, ease-out)'
        el.style.transform = ''
      }
    })
    const done = (e: TransitionEvent): void => {
      const el = e.target as HTMLElement
      if (e.propertyName !== 'transform' || !moving.includes(el)) return
      el.style.transition = ''
      el.removeAttribute('data-row-lift')
    }
    box.addEventListener('transitionend', done)
    const fallback = setTimeout(() => {
      for (const el of moving) {
        el.style.transition = ''
        el.style.transform = ''
        el.removeAttribute('data-row-lift')
      }
    }, 800)
    return () => {
      cancelAnimationFrame(raf)
      clearTimeout(fallback)
      box.removeEventListener('transitionend', done)
      for (const el of moving) {
        el.style.transition = ''
        el.style.transform = ''
        el.removeAttribute('data-row-lift')
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, enabled, groupId])
}
