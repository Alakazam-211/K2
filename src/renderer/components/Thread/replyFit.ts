// A quoted line cut to its slot in whole words, then "…": the reply bar on
// the message box and the quote strip inside a sent reply both use it, so a
// quote never stops mid-word.

import { useLayoutEffect, useRef, useState, type RefObject } from 'react'

/** `line` cut to whole words that fit `room` px (measured by `width`), then
 *  "…"; just "…" when not even the first word fits. `room` 0: not measured
 *  yet, the whole line. */
export function fitReplyWords(line: string, room: number, width: (s: string) => number): string {
  if (room <= 0 || width(line) <= room) return line
  const words = line.split(/\s+/).filter(Boolean)
  for (let n = words.length - 1; n > 0; n--) {
    const s = `${words.slice(0, n).join(' ').replace(/[\s,;:.–—-]+$/, '')}…`
    if (width(s) <= room) return s
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
 *  and refitted when the slot changes size. */
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
