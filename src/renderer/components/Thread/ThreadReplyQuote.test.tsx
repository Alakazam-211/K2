// @vitest-environment jsdom
// The quote strip inside a sent reply. Its words are the bubble's own text
// colour at full strength: in your own bubble that is on-accent over the
// accent fill, a 4.5:1 pair in every look, and any fade (opacity) on the
// words drops it under 4.5:1. The line is cut in whole words, then "…", as
// the reply bar's is, never mid-word.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
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
})
