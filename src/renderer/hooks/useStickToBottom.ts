// Stick-to-bottom for a scrolling message list (Zen conversation; Rosson
// 2026-10-04, the "working dots vanish when I type" and "messages don't move
// up with the box" bugs).
//
// The rule:
//   - At the bottom (within `threshold` px), the list stays pinned to its
//     TRUE bottom: `scrollHeight - clientHeight`, so whatever sits last (the
//     working dots too) stays in view. That holds when messages arrive, when
//     the list's own box shrinks or grows (a composer growing under it), and
//     when content inside changes size.
//   - Scrolled up, nothing snaps: size changes and new messages leave
//     `scrollTop` alone (top-anchored), so what the person reads stays put.
//   - "At bottom" changes only on a scroll the person made. A scroll event
//     that arrives after the box or content changed size (the browser
//     clamping `scrollTop`) is layout, not the person, and never unpins.
//
// Sizes are watched with a ResizeObserver on the list, its parent, each child
// (content growth), and any `extraTargets` (the composer). Callers also pass
// `deps` so a render that changes content re-pins before paint.

import { useCallback, useEffect, useLayoutEffect, useRef, type DependencyList, type UIEvent } from 'react'

export interface StickToBottomOptions {
  /** Distance from the bottom that still counts as "at the bottom". */
  threshold?: number
  /** Re-sync before paint when any of these change (items, working…). */
  deps: DependencyList
  /** More elements whose size changes should re-sync (the composer). */
  extraTargets?: () => Array<Element | null | undefined>
  /** The person scrolled within 16 px of the top (load older). */
  onNearTop?: () => void
}

export interface StickToBottom<T extends HTMLElement> {
  ref: React.RefObject<T | null>
  onScroll: (e: UIEvent<T>) => void
  /** Pin to the bottom now (a send, a conversation switch). */
  scrollToBottom: () => void
  /** Call right before older content is added on top: keeps the reading
   *  position when it lands. */
  holdForPrepend: () => void
  isAtBottom: () => boolean
}

/** True bottom of a scroll box. */
export const scrollBottomOf = (el: HTMLElement): number => Math.max(0, el.scrollHeight - el.clientHeight)

export function useStickToBottom<T extends HTMLElement>({
  threshold = 32,
  deps,
  extraTargets,
  onNearTop,
}: StickToBottomOptions): StickToBottom<T> {
  const ref = useRef<T | null>(null)
  const atBottom = useRef(true)
  const prependBase = useRef<number | null>(null)
  // The metrics at the last sync: a scroll event with different ones is
  // layout (a resize clamp), not the person.
  const seen = useRef<{ client: number; scroll: number } | null>(null)
  const observer = useRef<ResizeObserver | null>(null)
  const observed = useRef<Set<Element>>(new Set())
  const extraRef = useRef(extraTargets)
  extraRef.current = extraTargets
  const nearTopRef = useRef(onNearTop)
  nearTopRef.current = onNearTop

  const sync = useCallback((el: HTMLElement) => {
    // A hidden or not-yet-laid-out box says nothing about the bottom.
    if (el.clientHeight === 0) return
    if (prependBase.current !== null) {
      // Older content landed on top: move by exactly what it added. Until
      // it lands (a resize while loading), leave the position alone.
      if (el.scrollHeight !== prependBase.current) {
        el.scrollTop += el.scrollHeight - prependBase.current
        prependBase.current = null
      }
    } else if (atBottom.current) {
      const bottom = scrollBottomOf(el)
      if (el.scrollTop !== bottom) el.scrollTop = bottom
    }
    seen.current = { client: el.clientHeight, scroll: el.scrollHeight }
  }, [])

  const observeAll = useCallback(() => {
    const el = ref.current
    const ro = observer.current
    if (!el || !ro) return
    const want = new Set<Element>([el])
    if (el.parentElement) want.add(el.parentElement)
    for (const child of Array.from(el.children)) want.add(child)
    for (const extra of extraRef.current?.() ?? []) if (extra) want.add(extra)
    for (const t of observed.current) {
      if (!want.has(t)) {
        ro.unobserve(t)
        observed.current.delete(t)
      }
    }
    for (const t of want) {
      if (!observed.current.has(t)) {
        ro.observe(t)
        observed.current.add(t)
      }
    }
  }, [])

  useEffect(() => {
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      const el = ref.current
      if (el) sync(el)
    })
    observer.current = ro
    const seenSet = observed.current
    observeAll()
    return () => {
      ro.disconnect()
      observer.current = null
      seenSet.clear()
    }
  }, [observeAll, sync])

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    sync(el)
    observeAll()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps)

  const onScroll = useCallback(
    (e: UIEvent<T>) => {
      const el = e.currentTarget
      if (el.clientHeight === 0) return
      const last = seen.current
      if (!last || last.client !== el.clientHeight || last.scroll !== el.scrollHeight) {
        // The box or its content changed size since we last looked: this
        // scroll is the browser's clamp, not the person.
        sync(el)
        return
      }
      atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight <= threshold
      if (el.scrollTop <= 16) nearTopRef.current?.()
    },
    [sync, threshold],
  )

  const scrollToBottom = useCallback(() => {
    atBottom.current = true
    prependBase.current = null
    const el = ref.current
    if (el) sync(el)
  }, [sync])

  const holdForPrepend = useCallback(() => {
    const el = ref.current
    if (el) prependBase.current = el.scrollHeight
  }, [])

  const isAtBottom = useCallback(() => atBottom.current, [])

  return { ref, onScroll, scrollToBottom, holdForPrepend, isAtBottom }
}
