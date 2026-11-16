// Text measured as painted: the lanes size every label (clip names, ruler
// tags) with the font the page actually paints it in, read from a hidden
// probe styled like the label. Where nothing paints (jsdom in tests) a
// width per character stands in, so layout rules still run.

import { useLayoutEffect, useRef, useState } from 'react'

/** jsdom paints nothing (and has no canvas): tests measure by characters. */
export const PAINTS = typeof navigator !== 'undefined' && !/jsdom/i.test(navigator.userAgent) && typeof document !== 'undefined'

let ctx: CanvasRenderingContext2D | null | undefined

export function measurer(font: string | null, perChar: number): (text: string) => number {
  if (PAINTS && font) {
    if (ctx === undefined) ctx = document.createElement('canvas').getContext('2d')
    const c = ctx
    if (c)
      return (t) => {
        c.font = font
        return c.measureText(t).width
      }
  }
  return (t) => t.length * perChar
}

/**
 * The painted fonts of the probes (by class), read after each render and
 * again once the web fonts load, so a look change or a late font re-lays
 * the labels.
 */
export function useProbeFonts(classes: readonly string[]): { ref: React.RefObject<HTMLDivElement | null>; fonts: (string | null)[] } {
  const ref = useRef<HTMLDivElement>(null)
  const [fonts, setFonts] = useState<(string | null)[]>(() => classes.map(() => null))
  const [, bump] = useState(0)
  useLayoutEffect(() => {
    const box = ref.current
    if (!box || !PAINTS) return
    const next = classes.map((c) => {
      const el = box.querySelector(`.${c}`)
      if (!el) return null
      const cs = getComputedStyle(el)
      return cs.font || `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
    })
    if (next.some((f, i) => f !== fonts[i])) setFonts(next)
  })
  useLayoutEffect(() => {
    if (!PAINTS) return
    let live = true
    void document.fonts?.ready.then(() => live && bump((n) => n + 1))
    // A look change (data-style, data-palette on <html>) changes the fonts.
    const mo = typeof MutationObserver === 'undefined' ? null : new MutationObserver(() => bump((n) => n + 1))
    mo?.observe(document.documentElement, { attributes: true, attributeFilter: ['data-style', 'data-palette', 'data-scheme'] })
    return () => {
      live = false
      mo?.disconnect()
    }
  }, [])
  return { ref, fonts }
}
