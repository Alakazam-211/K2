// Who else is typing in this agent Thread, as a sliver under the message
// box: one line of the smallest type (`--type-xs`), muted, a few px off the
// box, lined up with the "+" glyph: "Mira is typing…", "Mira and Ben are
// typing…", "Several people are typing…" (threadTypingWords). The "…"
// carries the motion's meaning, so there are no dots after it. A narrow
// box drops whole words first.
//
// The row is always there at one height, empty while nobody types, so the
// box never moves when someone starts or stops. The words are one polite
// live region, read whole however the line is cut.

import { useLayoutEffect, useRef, useState, type JSX } from 'react'
import { threadTypingWords } from '@/lib/thread-typing'
import { fitReplyWords } from './ThreadReplyBar'

/** The sliver's height (px): one line of `--type-xs`. */
export const THREAD_TYPING_SLIVER_PX = 14
/** Its gap under the box (px). */
export const THREAD_TYPING_SLIVER_GAP_PX = 3
/** The words fitted to `room` px in whole words; empty for nobody. */
export function typingSliverText(names: readonly string[], room: number, width: (s: string) => number): string {
  if (names.length === 0) return ''
  return fitReplyWords(threadTypingWords(names), room, width)
}

/** Text width in `el`'s own font (a canvas; null where there is none). */
function measurer(el: HTMLElement): ((s: string) => number) | null {
  const ctx = typeof document !== 'undefined' ? document.createElement('canvas').getContext?.('2d') : null
  if (!ctx) return null
  const cs = getComputedStyle(el)
  ctx.font = cs.font || `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
  return (s) => ctx.measureText(s).width
}

export function ThreadTypingSliver({
  names,
  look,
}: {
  names: readonly string[]
  /** `zen`: the Thread's box; `square`: Terminal view's bar. */
  look: 'zen' | 'square'
}): JSX.Element {
  const rowRef = useRef<HTMLDivElement>(null)
  const [room, setRoom] = useState(0)
  useLayoutEffect(() => {
    const row = rowRef.current
    if (!row) return
    const read = (): void => {
      const cs = getComputedStyle(row)
      setRoom(Math.floor(row.clientWidth - (parseFloat(cs.paddingLeft) || 0) - (parseFloat(cs.paddingRight) || 0)))
    }
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(row)
    return () => ro.disconnect()
  }, [])
  const row = rowRef.current
  const width = row && room > 0 ? measurer(row) : null
  const words = threadTypingWords(names)
  const shown = width ? typingSliverText(names, room, width) : words
  return (
    <div
      ref={rowRef}
      data-testid="thread-typing-sliver"
      data-typing-sliver={look}
      data-typing={names.length ? '' : undefined}
      className="flex min-w-0 flex-none items-center"
      style={{
        height: THREAD_TYPING_SLIVER_PX,
        // The square bar's column already spaces its rows 6px apart.
        marginTop: look === 'zen' ? THREAD_TYPING_SLIVER_GAP_PX : THREAD_TYPING_SLIVER_GAP_PX - 6,
        // The "+" glyph's inset: the Thread box's 10px, the square bar's 6px.
        padding: look === 'zen' ? '0 3px 0 10px' : '0 3px 0 6px',
        fontSize: 'var(--type-xs, 10px)',
        lineHeight: `${THREAD_TYPING_SLIVER_PX}px`,
        color: look === 'zen' ? 'var(--zen-text-muted)' : 'var(--color-text-muted)',
      }}
    >
      <span aria-live="polite" className="sr-only">
        {words}
      </span>
      <span aria-hidden className="min-w-0 overflow-hidden whitespace-nowrap" data-typing-words="">
        {shown}
      </span>
    </div>
  )
}
