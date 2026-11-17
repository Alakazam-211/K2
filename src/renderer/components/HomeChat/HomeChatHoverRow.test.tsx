// @vitest-environment jsdom
// The hover row on a Home chat message (React, Reply, More):
//   1. Placement: on the bubble's top corner away from the sender (top right
//      on theirs, top left on yours), a little over its top edge, the same
//      place on every bubble; flipped inward at the column's edge.
//   2. It appears without flicker: one hover zone (the message, row
//      inside), a short delay before it hides, measured once as it appears
//      and hidden until placed; one message's row at a time.
//   3. No layout shift: the row, the picker and the who-reacted tip are
//      out of the flow, so showing them never moves the message column.
//   4. Motion: it fades and rises a few px in on the fast motion token, and
//      out a little faster; none under reduced motion.
//   5. Keyboard: Tab reaches a message, which shows its row; Tab goes on
//      to React, Reply, More (arrows move along them too); Esc goes back
//      to the message; the ring is the shared k2-focus one.
//   6. A jumped-to message gets a wash, not a flash.

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { HOVER_ROW_HIDE_MS } from './useHoverReveal'
import { BAR_OVER_X, BAR_OVER_Y, HomeChatMessageRow, actionBarPlace, type HomeChatRowActions } from './HomeChatMessageRow'
import type { HomeChatMessage } from '@/lib/home-chat/api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

const BAR = { width: 86, height: 34 }

describe('where the hover row goes', () => {
  it("theirs: over the bubble's top right corner, reaching outward", () => {
    const p = actionBarPlace({ width: 600 }, { left: 0, right: 120, top: 20 }, BAR, false)
    expect(p).toEqual({ top: 20 - BAR.height + BAR_OVER_Y, inset: 120 - BAR_OVER_X, inward: false })
    // It overlaps the bubble's top edge only by its top padding's worth, so
    // it never covers the first line.
    expect(p.top + BAR.height - 20).toBe(BAR_OVER_Y)
    expect(BAR_OVER_Y).toBeLessThanOrEqual(9)
  })

  it('a long bubble gets it in the same place as a short one: fixed to the top edge', () => {
    const short = actionBarPlace({ width: 600 }, { left: 0, right: 120, top: 20, bottom: 58 }, BAR, false)
    const tall = actionBarPlace({ width: 600 }, { left: 0, right: 120, top: 20, bottom: 420 }, BAR, false)
    expect(tall).toEqual(short)
    // Wider (still room outside): the same height, at its own corner.
    const wide = actionBarPlace({ width: 600 }, { left: 0, right: 470, top: 20, bottom: 420 }, BAR, false)
    expect(wide).toEqual({ top: short.top, inset: 470 - BAR_OVER_X, inward: false })
  })

  it("yours: over the bubble's top left corner (inset from the row's right edge)", () => {
    const p = actionBarPlace({ width: 600 }, { left: 480, right: 600, top: 0 }, BAR, true)
    expect(p).toEqual({ top: -BAR.height + BAR_OVER_Y, inset: 120 - BAR_OVER_X, inward: false })
    // A long one of yours: the same height.
    expect(actionBarPlace({ width: 600 }, { left: 140, right: 600, top: 0, bottom: 300 }, BAR, true)).toEqual({ top: p.top, inset: 460 - BAR_OVER_X, inward: false })
  })

  it('near the column edge it flips inward: over the top edge, inside the corner', () => {
    // Theirs, ending 60px from the column's far edge: no room outward.
    const p = actionBarPlace({ width: 600 }, { left: 0, right: 540, top: 20 }, BAR, false)
    expect(p.inward).toBe(true)
    expect(p.inset + BAR.width).toBeLessThanOrEqual(540)
    expect(p.inset).toBeGreaterThan(0)
    // Yours reaching the left edge: the same, mirrored.
    const q = actionBarPlace({ width: 600 }, { left: 10, right: 600, top: 20 }, BAR, true)
    expect(q.inward).toBe(true)
    expect(q.inset + BAR.width).toBeLessThanOrEqual(590)
    // Neither ever leaves the column.
    for (const x of [p, q]) {
      expect(x.inset).toBeGreaterThanOrEqual(0)
      expect(x.inset + BAR.width).toBeLessThanOrEqual(600)
    }
  })

  it('a narrow column keeps it inside: never clipped at either side', () => {
    const p = actionBarPlace({ width: 100 }, { left: 0, right: 96, top: 20 }, BAR, false)
    expect(p.inward).toBe(true)
    expect(p.inset).toBeGreaterThanOrEqual(0)
    expect(p.inset + BAR.width).toBeLessThanOrEqual(100)
    // Narrower than the bar: flush with the sender's side.
    expect(actionBarPlace({ width: 70 }, { left: 0, right: 70, top: 20 }, BAR, false).inset).toBe(0)
  })
})

const msg: HomeChatMessage = { id: '5', conv: '7', authorId: '2', body: 'hello', createdAt: 1, seq: 5, reactions: [], replyTo: null, deleted: false, nonce: null }
const actions: HomeChatRowActions = { react: vi.fn(), reply: vi.fn(), jump: vi.fn(), who: async () => [] }
function row(o: Partial<{ flash: boolean; readOnly: boolean; mine: boolean; m: HomeChatMessage }> = {}) {
  return (
    <HomeChatMessageRow
      m={o.m ?? msg}
      mine={o.mine ?? false}
      author="Ana"
      timeLabel="now"
      still
      runStart
      tail
      reply={null}
      flash={o.flash ?? false}
      readOnly={o.readOnly ?? false}
      actions={actions}
    />
  )
}

/** Rects for a list of rows 80px apart, each with a 200px bubble 20px down. */
function mockRects(): { bubbleReads: () => number } {
  let reads = 0
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const row = this.closest<HTMLElement>('[data-home-chat-message]')
    const y = 100 + 80 * Number(row?.getAttribute('data-zen-message') ?? 0)
    if (this.hasAttribute('data-home-chat-message')) return DOMRect.fromRect({ x: 0, y, width: 600, height: 80 })
    if (this.hasAttribute('data-zen-bubble')) {
      reads++
      return DOMRect.fromRect({ x: 0, y: y + 20, width: 200, height: 40 })
    }
    return DOMRect.fromRect({ x: 0, y: 0, width: 0, height: 0 })
  })
  return { bubbleReads: () => reads }
}

const barOf = (message: Element): HTMLElement => message.querySelector('[data-home-chat-actions]') as HTMLElement
const isShown = (message: Element): boolean => barOf(message).hasAttribute('data-shown') && barOf(message).style.visibility === 'visible'

describe('the hover row appears without flicker', () => {
  it('is inside the message (one hover zone), and hidden until placed', () => {
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    expect(message.contains(barOf(message))).toBe(true)
    // Never shown (nor reachable) before it has a place.
    expect(barOf(message).style.visibility).toBe('hidden')
    expect(barOf(message).style.pointerEvents).toBe('none')
    act(() => {
      fireEvent.pointerEnter(message, { pointerType: 'mouse' })
    })
    expect(isShown(message)).toBe(true)
  })

  it('leaving hides it only after a short delay; coming back keeps it, unmoved', () => {
    vi.useFakeTimers()
    try {
      const rects = mockRects()
      const { container } = render(row())
      const message = container.querySelector('[data-home-chat-message]') as HTMLElement
      act(() => {
        fireEvent.pointerEnter(message, { pointerType: 'mouse' })
      })
      const at = { left: barOf(message).style.left, top: barOf(message).style.top }
      expect(rects.bubbleReads()).toBe(1)
      expect(HOVER_ROW_HIDE_MS).toBeGreaterThanOrEqual(120)
      expect(HOVER_ROW_HIDE_MS).toBeLessThanOrEqual(200)
      act(() => {
        fireEvent.pointerLeave(message, { pointerType: 'mouse' })
        vi.advanceTimersByTime(HOVER_ROW_HIDE_MS - 40)
      })
      expect(isShown(message)).toBe(true)
      // Back in before the delay ends (an edge crossed): no blink, no re-measure.
      act(() => {
        fireEvent.pointerEnter(message, { pointerType: 'mouse' })
        fireEvent.pointerMove(message, { pointerType: 'mouse' })
        vi.advanceTimersByTime(HOVER_ROW_HIDE_MS * 2)
      })
      expect(isShown(message)).toBe(true)
      expect(rects.bubbleReads()).toBe(1)
      expect({ left: barOf(message).style.left, top: barOf(message).style.top }).toEqual(at)
      act(() => {
        fireEvent.pointerLeave(message, { pointerType: 'mouse' })
        vi.advanceTimersByTime(HOVER_ROW_HIDE_MS)
      })
      expect(isShown(message)).toBe(false)
    } finally {
      vi.useRealTimers()
    }
  })

  it('moving to another message hides the last one at once: one row at a time', () => {
    mockRects()
    const { container } = render(
      <>
        {row({ m: { ...msg, id: '1' } })}
        {row({ m: { ...msg, id: '2' } })}
      </>,
    )
    const [a, b] = Array.from(container.querySelectorAll('[data-home-chat-message]'))
    act(() => {
      fireEvent.pointerEnter(a, { pointerType: 'mouse' })
    })
    act(() => {
      fireEvent.pointerLeave(a, { pointerType: 'mouse' })
      fireEvent.pointerEnter(b, { pointerType: 'mouse' })
    })
    expect(isShown(a)).toBe(false)
    expect(isShown(b)).toBe(true)
  })

  it('a touch shows nothing (a long press opens the menu instead)', () => {
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    act(() => {
      fireEvent.pointerEnter(message, { pointerType: 'touch' })
    })
    expect(isShown(message)).toBe(false)
  })
})

/** What lays the message out: its in-flow children, as drawn. */
function flow(message: Element): string[] {
  return Array.from(message.children)
    .filter((el) => !/\b(absolute|fixed)\b/.test(el.className) && !['absolute', 'fixed'].includes((el as HTMLElement).style.position))
    .map((el) => `${el.tagName} ${el.getAttribute('style') ?? ''} ${el.childElementCount}`)
}

describe('showing the row, the picker or the tip never moves the column', () => {
  it('the message lays out the same, and what appears is out of the flow', async () => {
    mockRects()
    const reacted = { ...msg, reactions: [{ emoji: '👍', count: 2, me: false }] }
    const { container } = render(
      <div data-testid="column">
        {row({ m: { ...reacted, id: '1' } })}
        {row({ m: { ...msg, id: '2' } })}
      </div>,
    )
    const column = screen.getByTestId('column')
    const [a, b] = Array.from(container.querySelectorAll('[data-home-chat-message]'))
    const before = [flow(a), flow(b), column.childElementCount]
    act(() => {
      fireEvent.pointerEnter(a, { pointerType: 'mouse' })
    })
    expect(isShown(a)).toBe(true)
    expect(barOf(a).className).toMatch(/\babsolute\b/)
    expect([flow(a), flow(b), column.childElementCount]).toEqual(before)
    // The picker: portalled to the page, fixed.
    await act(async () => {
      fireEvent.click(a.querySelector('[data-testid="home-chat-react"]') as HTMLElement)
    })
    const picker = screen.getByTestId('home-chat-reaction-picker')
    expect(column.contains(picker)).toBe(false)
    expect(picker.closest('.fixed')).not.toBeNull()
    expect([flow(a), flow(b), column.childElementCount]).toEqual(before)
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('home-chat-emoji-search'), { key: 'Escape' })
    })
    // The who-reacted tip: portalled too.
    await act(async () => {
      fireEvent.mouseEnter(a.querySelector('[data-testid="home-chat-reaction"]') as HTMLElement)
    })
    const tip = screen.getByTestId('home-chat-reaction-tip')
    expect(column.contains(tip)).toBe(false)
    expect(tip.className).toMatch(/\bfixed\b/)
    expect([flow(a), flow(b), column.childElementCount]).toEqual(before)
  })
})

function mockReducedMotion(reduce: boolean): void {
  vi.stubGlobal(
    'matchMedia',
    vi.fn((q: string) => ({ matches: reduce && q.includes('reduce'), media: q, addEventListener: () => undefined, removeEventListener: () => undefined })),
  )
}

describe('the hover row moves on the motion tokens', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('fades and rises in on the fast token; fades out faster, then leaves the tab order', () => {
    mockReducedMotion(false)
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    const hidden = barOf(message).style
    expect(hidden.transform).toMatch(/translateY\([2-4]px\)/)
    expect(hidden.transition).toContain('opacity calc(var(--motion-duration-fast) * 0.75)')
    // Visibility waits for the fade, so the fade is seen.
    expect(hidden.transition).toContain('visibility 0s linear calc(var(--motion-duration-fast) * 0.75)')
    act(() => {
      fireEvent.pointerEnter(message, { pointerType: 'mouse' })
    })
    const shown = barOf(message).style
    expect(shown.transform).toBe('none')
    expect(shown.transition).toContain('opacity var(--motion-duration-fast) var(--motion-ease-standard)')
    expect(shown.transition).toContain('transform var(--motion-duration-fast) var(--motion-ease-standard)')
    expect(shown.transition).not.toContain('visibility')
  })

  it('under reduced motion it just appears and goes', () => {
    mockReducedMotion(true)
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    expect(barOf(message).style.transition).toBe('none')
    expect(barOf(message).style.transform).toBe('none')
    act(() => {
      fireEvent.pointerEnter(message, { pointerType: 'mouse' })
    })
    expect(barOf(message).style.transition).toBe('none')
    expect(barOf(message).style.transform).toBe('none')
  })
})

describe('the hover row by keyboard', () => {
  it('a message takes focus (the shared ring) and focus shows its row', () => {
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    expect(message.tabIndex).toBe(0)
    expect(message.getAttribute('role')).toBe('article')
    expect(message.getAttribute('aria-label')).toBe('Message from Ana')
    expect(message.className).toMatch(/(^| )k2-focus( |$)/)
    act(() => message.focus())
    expect(isShown(message)).toBe(true)
    // Focus leaving the message hides it.
    const out = document.createElement('button')
    document.body.append(out)
    act(() => out.focus())
    expect(isShown(message)).toBe(false)
    out.remove()
  })

  it("a click's focus doesn't pin the row once the pointer has gone", () => {
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    // Long before any other test's clock, so this press can't touch theirs.
    vi.spyOn(performance, 'now').mockReturnValue(-1e9)
    act(() => {
      fireEvent.pointerDown(message, { pointerType: 'mouse' })
      message.focus()
    })
    expect(isShown(message)).toBe(false)
  })

  it('Tab order: the message, then React, Reply, More, then what is in the bubble and the chips', () => {
    mockRects()
    const reacted = { ...msg, reactions: [{ emoji: '👍', count: 1, me: false }] }
    const { container } = render(row({ m: reacted }))
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    const tabbable = Array.from(message.querySelectorAll<HTMLElement>('button, [tabindex]')).filter((el) => el.tabIndex >= 0)
    expect(tabbable.map((el) => el.getAttribute('aria-label') ?? el.getAttribute('data-testid'))).toEqual(['React', 'Reply', 'More', '👍 1'])
    const bubble = message.querySelector('[data-zen-bubble]') as HTMLElement
    expect(screen.getByTestId('home-chat-more').compareDocumentPosition(bubble) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('arrows move along the row; Esc goes back to the message, which keeps its row', () => {
    mockRects()
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    act(() => message.focus())
    const react = screen.getByTestId('home-chat-react')
    act(() => react.focus())
    fireEvent.keyDown(react, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(screen.getByTestId('home-chat-reply'))
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'End' })
    expect(document.activeElement).toBe(screen.getByTestId('home-chat-more'))
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(react)
    const outer = vi.fn()
    document.addEventListener('keydown', outer)
    act(() => {
      fireEvent.keyDown(react, { key: 'Escape' })
    })
    document.removeEventListener('keydown', outer)
    expect(document.activeElement).toBe(message)
    expect(outer).not.toHaveBeenCalled()
    expect(isShown(message)).toBe(true)
  })

  it('a deleted message has no row and is not a stop', () => {
    const { container } = render(row({ m: { ...msg, deleted: true, body: '' } }))
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    expect(message.hasAttribute('tabindex')).toBe(false)
    expect(message.querySelector('[data-home-chat-actions]')).toBeNull()
  })
})

describe('the hover row on a message', () => {
  it('React is an icon button (no text glyph); the row is a toolbar of React, Reply, More', () => {
    const { container } = render(row())
    const react = screen.getByTestId('home-chat-react')
    expect(react.querySelector('svg')).not.toBeNull()
    expect(react.textContent).toBe('')
    const bar = container.querySelector('[role="toolbar"]') as HTMLElement
    expect(bar.getAttribute('aria-label')).toBe('Message actions')
    expect([...bar.querySelectorAll('button')].map((b) => b.getAttribute('aria-label'))).toEqual(['React', 'Reply', 'More'])
  })

  it('is placed from the bubble it belongs to (measured rects)', () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.hasAttribute('data-home-chat-message')) return DOMRect.fromRect({ x: 0, y: 100, width: 600, height: 80 })
      if (this.hasAttribute('data-zen-bubble')) return DOMRect.fromRect({ x: 0, y: 120, width: 200, height: 40 })
      return DOMRect.fromRect({ x: 0, y: 0, width: 0, height: 0 })
    })
    const { container } = render(row())
    const message = container.querySelector('[data-home-chat-message]') as HTMLElement
    act(() => {
      fireEvent.pointerEnter(message, { pointerType: 'mouse' })
    })
    const holder = container.querySelector('[data-home-chat-actions]') as HTMLElement
    expect(holder.style.left).toBe(`${200 - BAR_OVER_X}px`)
    expect(Number.parseFloat(holder.style.top)).toBeLessThan(20)
    expect(holder.hasAttribute('data-inward')).toBe(false)
  })

  it('React opens the picker; Esc closes it and puts focus back on React', async () => {
    render(row())
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-react'))
    })
    expect(screen.getByTestId('home-chat-reaction-picker')).toBeTruthy()
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('home-chat-emoji-search'), { key: 'Escape' })
    })
    expect(screen.queryByTestId('home-chat-reaction-picker')).toBeNull()
    expect(document.activeElement).toBe(screen.getByTestId('home-chat-react'))
  })

  it('a jumped-to message gets a soft accent wash', () => {
    const { rerender, container } = render(row())
    expect(container.querySelector('[data-home-chat-wash]')).toBeNull()
    rerender(row({ flash: true }))
    const wash = container.querySelector<HTMLElement>('[data-home-chat-wash]')!
    expect(wash.style.background).toContain('var(--zen-accent) 18%')
  })
})
