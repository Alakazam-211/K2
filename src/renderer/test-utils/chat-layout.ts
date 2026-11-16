// A fake layout for the chats that draw the Thread's rows (the Thread,
// Chatter, Home chat, project chat), frame by frame the way a browser runs
// them: the scroll event (when the position moved), requestAnimationFrame,
// then ResizeObserver (when the rows' column changed size). jsdom lays
// nothing out, so the getters compute offsets from the DOM the chat drew:
// the scroller is the one element with `data-zen-conversation-body`, the
// rows' column is its `[data-thread-column]`, rows (`data-zen-message`)
// are `rowPx(key)` tall, spacers their inline height and margins, "Earlier
// messages" (`data-zen-load-older`) `olderPx`; everything else is 0.
// Offsets are rounded the way a browser rounds them.
//
//   beforeAll(() => installChatLayout({ view: 900, rowPx: () => 40 }))
//   afterAll(uninstallChatLayout)
//   beforeEach(() => { vi.useFakeTimers(); stubChatFrames() })
//   afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers() })

import { vi } from 'vitest'
import { act, fireEvent } from '@testing-library/react'

export interface ChatLayoutOptions {
  /** The scroller's height (px). */
  view: number
  rowPx: (key: string) => number
  /** "Earlier messages" (px). */
  olderPx?: number
}

/** The scroll position, and scrollTop writes by the code that moved it
 *  (the reader's own, `readerScroll`, are not counted). */
export const chatLayout = { scrollTop: 0, codeMoves: 0 }

let opts: ChatLayoutOptions = { view: 600, rowPx: () => 40 }
let readerWrite = false
let lastEventTop = -1
let lastColumnPx = -1
let rafQ: Array<{ id: number; cb: FrameRequestCallback }> = []
let rafId = 0

function isColumn(el: Element | null): el is HTMLElement {
  return !!el && (el as HTMLElement).hasAttribute?.('data-thread-column')
}
function isScroller(el: Element): boolean {
  return !!(el as HTMLElement).hasAttribute?.('data-zen-conversation-body')
}
function heightOf(el: HTMLElement): number {
  if (el.hasAttribute('data-win-spacer')) return parseFloat(el.style.height) || 0
  if (el.hasAttribute('data-zen-message')) return opts.rowPx(el.getAttribute('data-zen-message') ?? '')
  if (el.hasAttribute('data-zen-load-older')) return opts.olderPx ?? 30
  return 0
}
function marginOf(el: HTMLElement, side: 'top' | 'bottom'): number {
  return parseFloat(side === 'top' ? el.style.marginTop : el.style.marginBottom) || 0
}
/** Each child's top in the column, and the column's height. */
function layout(col: HTMLElement): { tops: Map<Element, number>; height: number } {
  const gap = parseFloat(col.style.gap) || 0
  const tops = new Map<Element, number>()
  let pos = 0
  const kids = Array.from(col.children) as HTMLElement[]
  kids.forEach((el, i) => {
    pos += marginOf(el, 'top')
    tops.set(el, pos)
    pos += heightOf(el) + marginOf(el, 'bottom')
    if (i < kids.length - 1) pos += gap
  })
  return { tops, height: pos }
}
function columnPx(scroller: HTMLElement): number {
  const c = scroller.querySelector('[data-thread-column]')
  return isColumn(c) ? layout(c).height : 0
}

const PROPS = ['offsetTop', 'offsetHeight', 'clientHeight', 'scrollHeight', 'scrollTop'] as const
const saved: Record<string, PropertyDescriptor | undefined> = {}

export function installChatLayout(o: ChatLayoutOptions): void {
  opts = o
  const proto = HTMLElement.prototype
  for (const k of PROPS) saved[k] = Object.getOwnPropertyDescriptor(proto, k)
  saved.rect = Object.getOwnPropertyDescriptor(Element.prototype, 'getBoundingClientRect')
  Object.defineProperty(proto, 'offsetTop', {
    configurable: true,
    get(this: HTMLElement) {
      const p = this.parentElement
      return isColumn(p) ? Math.round(layout(p).tops.get(this) ?? 0) : 0
    },
  })
  Object.defineProperty(proto, 'offsetHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return Math.round(heightOf(this))
    },
  })
  Object.defineProperty(proto, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return isScroller(this) ? opts.view : 0
    },
  })
  Object.defineProperty(proto, 'scrollHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return isScroller(this) ? Math.round(columnPx(this)) : 0
    },
  })
  Object.defineProperty(proto, 'scrollTop', {
    configurable: true,
    get(this: HTMLElement) {
      return isScroller(this) ? chatLayout.scrollTop : 0
    },
    set(this: HTMLElement, v: number) {
      if (!isScroller(this)) return
      const next = Math.round(Math.max(0, Math.min(v, this.scrollHeight - opts.view)))
      if (!readerWrite && next !== chatLayout.scrollTop) chatLayout.codeMoves += 1
      chatLayout.scrollTop = next
    },
  })
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      const p = this.parentElement
      const top = isColumn(p) ? (layout(p).tops.get(this) ?? 0) - chatLayout.scrollTop : 0
      const height = isScroller(this) ? opts.view : heightOf(this)
      return { top, bottom: top + height, left: 0, right: 600, width: 600, height, x: 0, y: top, toJSON: () => ({}) }
    },
  })
}

export function uninstallChatLayout(): void {
  const proto = HTMLElement.prototype
  for (const k of PROPS) {
    const d = saved[k]
    if (d) Object.defineProperty(proto, k, d)
    else delete (proto as unknown as Record<string, unknown>)[k]
  }
  if (saved.rect) Object.defineProperty(Element.prototype, 'getBoundingClientRect', saved.rect)
}

export class FakeResizeObserver {
  static all: FakeResizeObserver[] = []
  observed: Element[] = []
  constructor(private readonly cb: ResizeObserverCallback) {
    FakeResizeObserver.all.push(this)
  }
  observe(el: Element): void {
    this.observed.push(el)
  }
  unobserve(el: Element): void {
    this.observed = this.observed.filter((o) => o !== el)
  }
  disconnect(): void {
    this.observed = []
  }
  fire(): void {
    this.cb([], this as unknown as ResizeObserver)
  }
}

/** Stub ResizeObserver and requestAnimationFrame (the frames run them) and
 *  reset the scroll state. Call after `vi.useFakeTimers()`. */
export function stubChatFrames(): void {
  FakeResizeObserver.all = []
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  rafQ = []
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    rafId += 1
    rafQ.push({ id: rafId, cb })
    return rafId
  })
  vi.stubGlobal('cancelAnimationFrame', (id: number) => {
    rafQ = rafQ.filter((r) => r.id !== id)
  })
  chatLayout.scrollTop = 0
  chatLayout.codeMoves = 0
  readerWrite = false
  lastEventTop = -1
  lastColumnPx = -1
}

/** The chat's scroller. */
export function chatScroller(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-zen-conversation-body]')
  if (!el) throw new Error('no chat scroller')
  return el
}

/** One frame: the scroll event (if the position moved), rAF, then resize. */
export function chatFrame(): void {
  act(() => {
    if (chatLayout.scrollTop !== lastEventTop) {
      lastEventTop = chatLayout.scrollTop
      fireEvent.scroll(chatScroller())
    }
  })
  act(() => {
    const q = rafQ
    rafQ = []
    for (const r of q) r.cb(performance.now())
  })
  act(() => {
    const px = columnPx(chatScroller())
    if (px !== lastColumnPx) {
      lastColumnPx = px
      for (const ro of FakeResizeObserver.all) if (ro.observed.length) ro.fire()
    }
  })
  act(() => {
    vi.advanceTimersByTime(16)
  })
}

export function chatFrames(n: number): void {
  for (let i = 0; i < n; i++) chatFrame()
}

/** The reader scrolls by `dy` px (a wheel tick, a fling's frame), then a frame. */
export function readerScroll(dy: number): void {
  readerWrite = true
  chatScroller().scrollTop = chatLayout.scrollTop + dy
  readerWrite = false
  chatFrame()
}

/** The drawn rows' keys, first..last. */
export function drawnRows(): string {
  const rows = Array.from(chatScroller().querySelectorAll('[data-thread-column] > [data-zen-message]'))
  return `${rows[0]?.getAttribute('data-zen-message')}..${rows[rows.length - 1]?.getAttribute('data-zen-message')}`
}

/** The first row on screen and where it is on screen. */
export function rowOnScreen(): { key: string; top: number } | null {
  for (const r of Array.from(chatScroller().querySelectorAll<HTMLElement>('[data-thread-column] > [data-zen-message]'))) {
    const b = r.getBoundingClientRect()
    if (b.bottom > 1 && b.top < opts.view) return { key: r.getAttribute('data-zen-message') ?? '', top: Math.round(b.top) }
  }
  return null
}

/** Where row `key` is on screen (null: not drawn). */
export function rowTop(key: string): number | null {
  const r = chatScroller().querySelector<HTMLElement>(`[data-thread-column] > [data-zen-message="${key}"]`)
  return r ? Math.round(r.getBoundingClientRect().top) : null
}
