// The Thread's message column, and whether it is narrow (Rosson 2026-10-18:
// "once the width of the messaging area gets too thin, can we allow the
// message cards to consume the full width of the thread space rather than
// the ratio/% of the width that it currently consumes?").
//
// `ThreadMessageColumn` is the list's flex column (overlay Thread, ticket
// chat, project chat). It measures its own width (a ResizeObserver: the
// column, not the window) and tells the bubbles inside whether it is
// narrow. Wide: a bubble keeps today's `min(78%, 46rem)`. Narrow: it may
// use the whole column but a small inset on the far side, so yours still
// reads as right and theirs as left.

import { createContext, useCallback, useContext, useLayoutEffect, useState } from 'react'
import type { CSSProperties, JSX, ReactNode, Ref } from 'react'

/** Below this column width (px), bubbles and cards use the full column. */
export const THREAD_NARROW_PX = 480
/** Leaving narrow takes this much more width, so a scrollbar coming and
 *  going at the edge can't flip the layout back and forth. */
export const THREAD_NARROW_HYSTERESIS_PX = 16
/** Narrow: the gap kept on the far side of a bubble (the side cue). */
export const THREAD_NARROW_INSET_PX = 12

/** A message's max width in a wide column (unchanged from 0.46.0). */
export const THREAD_BUBBLE_MAX_WIDTH = 'min(78%, 46rem)'
/** A message's max width in a narrow column. */
export const THREAD_BUBBLE_MAX_WIDTH_NARROW = `calc(100% - ${THREAD_NARROW_INSET_PX}px)`

/** Narrow after measuring `width`, given what it was. A 0 width (hidden,
 *  not laid out yet) keeps the old answer. */
export function threadColumnNarrowNext(width: number, wasNarrow: boolean): boolean {
  if (!(width > 0)) return wasNarrow
  if (wasNarrow) return width < THREAD_NARROW_PX + THREAD_NARROW_HYSTERESIS_PX
  return width < THREAD_NARROW_PX
}

const ThreadColumnNarrowContext = createContext(false)

/** True inside a narrow `ThreadMessageColumn`; false elsewhere (the Zen
 *  Garden widget and anything outside a column keep the ratio). */
export function useThreadColumnNarrow(): boolean {
  return useContext(ThreadColumnNarrowContext)
}

/** The max width for a message in this column. */
export function threadBubbleMaxWidth(narrow: boolean): string {
  return narrow ? THREAD_BUBBLE_MAX_WIDTH_NARROW : THREAD_BUBBLE_MAX_WIDTH
}

function setRef<T>(ref: Ref<T> | undefined, value: T | null): void {
  if (!ref) return
  if (typeof ref === 'function') ref(value)
  else (ref as { current: T | null }).current = value
}

/** The list's flex column of messages; measures itself. */
export function ThreadMessageColumn({
  children,
  gap,
  className = 'flex flex-col',
  style,
  ref,
  testId,
}: {
  children: ReactNode
  gap: number
  className?: string
  style?: CSSProperties
  ref?: Ref<HTMLDivElement>
  testId?: string
}): JSX.Element {
  const [el, setEl] = useState<HTMLDivElement | null>(null)
  const [narrow, setNarrow] = useState(false)
  const attach = useCallback(
    (node: HTMLDivElement | null) => {
      setEl(node)
      setRef(ref, node)
    },
    [ref],
  )
  // Layout effect: the first measure lands before paint (no wide flash).
  useLayoutEffect(() => {
    if (!el) return
    setNarrow((was) => threadColumnNarrowNext(el.getBoundingClientRect().width, was))
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1]
      if (!entry) return
      setNarrow((was) => threadColumnNarrowNext(entry.contentRect.width, was))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [el])
  return (
    <div
      ref={attach}
      className={className}
      style={{ gap, ...style }}
      data-thread-column=""
      data-thread-narrow={narrow ? '' : undefined}
      data-testid={testId}
    >
      <ThreadColumnNarrowContext.Provider value={narrow}>{children}</ThreadColumnNarrowContext.Provider>
    </div>
  )
}
