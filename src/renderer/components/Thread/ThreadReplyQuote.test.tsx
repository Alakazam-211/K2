// @vitest-environment jsdom
// The quote strip inside a sent reply. Its words are the bubble's own text
// colour at full strength: in your own bubble that is on-accent over the
// accent fill, a 4.5:1 pair in every look, and any fade (opacity) on the
// words drops it under 4.5:1. The line is cut in whole words, then "…", as
// the reply bar's is, never mid-word.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { ThreadReplyQuote } from './ThreadMessageMenu'

const reply = (quote: string) => ({ part: false, seq: 0, from: 'Ana Ruiz', id: 'm1', quote, rest: '' })

/** Every element from the strip to `el`, inclusive. */
function chain(strip: HTMLElement, el: HTMLElement): HTMLElement[] {
  const out: HTMLElement[] = []
  for (let n: HTMLElement | null = el; n; n = n.parentElement) {
    out.push(n)
    if (n === strip) break
  }
  return out
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the reply quote strip', () => {
  it('draws its words in the bubble text colour, never faded', () => {
    const { container } = render(<ThreadReplyQuote reply={reply('Can people pick a slot?')} />)
    const strip = container.querySelector<HTMLElement>('[data-thread-reply-quote]')!
    const text = strip.querySelector<HTMLElement>('[data-reply-quote-text]')!
    const who = strip.querySelector<HTMLElement>('[data-reply-quote-who]')!
    expect(text).not.toBeNull()
    expect(who).not.toBeNull()
    for (const el of [...chain(strip, text), ...chain(strip, who)]) {
      expect(el.style.opacity === '' || el.style.opacity === '1', el.outerHTML.slice(0, 60)).toBe(true)
      expect(el.style.color === '' || el.style.color === 'currentcolor' || el.style.color === 'inherit').toBe(true)
    }
    expect(strip.textContent).toBe('Ana Ruiz: Can people pick a slot?')
  })

  describe('fitted in whole words', () => {
    beforeEach(() => {
      // 7px a character; the line's slot is 100px.
      vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () => ({ font: '', measureText: (s: string) => ({ width: s.length * 7 }) }) as unknown as CanvasRenderingContext2D,
      )
      vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
        return this.hasAttribute('data-reply-quote-text') ? 100 : 0
      })
    })

    it('cuts a long line after a whole word, then "…"', () => {
      const { container } = render(<ThreadReplyQuote reply={reply('Can people pick a slot on the pricing page this week?')} />)
      const text = container.querySelector<HTMLElement>('[data-reply-quote-text]')!
      expect(text.textContent).toBe('Can people…')
      expect(text.textContent!.length * 7).toBeLessThanOrEqual(100)
    })

    it('keeps a line that fits whole', () => {
      const { container } = render(<ThreadReplyQuote reply={reply('Ship it?')} />)
      expect(container.querySelector('[data-reply-quote-text]')!.textContent).toBe('Ship it?')
    })
  })

  // The bubble sizes to its content. A short reply ("ok") leaves the
  // quote's slot as the widest thing in it, so if the slot's natural width
  // were the cut line, each cut would narrow the bubble and the next fit
  // would cut again, down to "Name: …". The slot's natural width is the
  // whole line, so the bubble grows (to its own max width) before cutting.
  describe('inside a bubble that sizes to its content', () => {
    const CHAR = 7.3 // px a character, fractional as real text is
    const LONG = 'Can people pick a slot on the pricing page this week?'
    const observers: Array<() => void> = []
    /** The slot's natural (max-content) width: its widest child, or the
     *  whole line it holds room for. */
    const natural = (el: HTMLElement): number => {
      const kids = [...el.childNodes].map((n) => (n.textContent ?? '').length)
      const room = (el.getAttribute('data-reply-quote-line') ?? '').length
      return Math.max(room, ...kids, 0) * CHAR
    }
    /** The slot: as wide as its content, up to 160px (a short reply). */
    const sizesToContent = (): void => {
      vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
        return this.hasAttribute('data-reply-quote-text') ? Math.floor(Math.min(160, natural(this))) : 0
      })
    }
    const settle = (): void => {
      for (let i = 0; i < 20; i++) act(() => observers.forEach((f) => f()))
    }
    beforeEach(() => {
      observers.length = 0
      vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () => ({ font: '', measureText: (s: string) => ({ width: s.length * CHAR }) }) as unknown as CanvasRenderingContext2D,
      )
      vi.stubGlobal(
        'ResizeObserver',
        class {
          constructor(private cb: () => void) {}
          observe(): void {
            observers.push(this.cb)
          }
          disconnect(): void {}
        },
      )
    })
    afterEach(() => vi.unstubAllGlobals())

    it('a short reply to a long line shows a useful part of the line, not "…" alone', () => {
      sizesToContent()
      const { container } = render(<ThreadReplyQuote reply={reply(LONG)} />)
      settle()
      const text = container.querySelector<HTMLElement>('[data-reply-quote-text]')!.textContent!
      expect(text).toBe('Can people pick a…')
      expect(text.length * CHAR).toBeLessThanOrEqual(160)
    })

    it('a long reply (the bubble already at its max width) cuts as before', () => {
      vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
        return this.hasAttribute('data-reply-quote-text') ? 160 : 0
      })
      const { container } = render(<ThreadReplyQuote reply={reply(LONG)} />)
      settle()
      expect(container.querySelector('[data-reply-quote-text]')!.textContent).toBe('Can people pick a…')
    })

    it('a line that fits whole stays whole (no cut from rounding)', () => {
      sizesToContent()
      const { container } = render(<ThreadReplyQuote reply={reply('Ship it on Friday?')} />)
      settle()
      expect(container.querySelector('[data-reply-quote-text]')!.textContent).toBe('Ship it on Friday?')
    })
  })
})
