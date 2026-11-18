// A quoted line cut to its slot in whole words, then "…": the reply bar on
// the message box and the quote strip inside a sent reply both use it, so a
// quote never stops mid-word.

import { useLayoutEffect, useRef, useState, type RefObject } from 'react'

/** `line` cut to whole words that fit `room` px (measured by `width`), then
 *  "…"; just "…" when not even the first word fits. `room` 0: not measured
 *  yet, the whole line. `room` is whole px (clientWidth rounds), so text up
 *  to a px over still fits: a slot exactly as wide as its line keeps it. */
export function fitReplyWords(line: string, room: number, width: (s: string) => number): string {
  const fits = (s: string): boolean => width(s) <= room + 1
  if (room <= 0 || fits(line)) return line
  const words = line.split(/\s+/).filter(Boolean)
  for (let n = words.length - 1; n > 0; n--) {
    const s = `${words.slice(0, n).join(' ').replace(/[\s,;:.–—-]+$/, '')}…`
    if (fits(s)) return s
  }
  return '…'
}

/** Text width in `el`'s own font (a canvas; null where there is none). */
function textWidth(el: HTMLElement): ((s: string) => number) | null {
  const ctx = typeof document !== 'undefined' ? document.createElement('canvas').getContext?.('2d') : null
  if (!ctx) return null
  const cs = getComputedStyle(el)
  ctx.font = cs.font || `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
  return (s) => ctx.measureText(s).width
}

/** The line, fitted to its slot (the span `ref` goes on) in whole words,
 *  and refitted when the slot changes size. The slot is read on mount and
 *  on resize only; an unchanged width sets the same state (no render).
 *  Where the slot's box sizes to its content (a bubble), the slot's natural
 *  width must not be the cut text, or each cut narrows the box and the next
 *  fit cuts again: `ThreadReplyQuote` keeps room for the whole line. */
export function useFittedLine(line: string): { ref: RefObject<HTMLSpanElement | null>; text: string } {
  const ref = useRef<HTMLSpanElement | null>(null)
  const [room, setRoom] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const read = (): void => setRoom(Math.floor(el.clientWidth))
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const el = ref.current
  const width = el && room > 0 ? textWidth(el) : null
  return { ref, text: width ? fitReplyWords(line, room, width) : line }
}
