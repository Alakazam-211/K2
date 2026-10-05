// @vitest-environment jsdom
//
// Zen conversation scroll (Rosson 2026-10-04):
//   A. Typing while the agent works must keep the working dots in view.
//   B. The growing message box pushes the list up while the person is at the
//      bottom; scrolled up, nothing moves.
//
// jsdom has no layout, so this file models the one that matters: a column of
// fixed height holding the list (flex: 1) over the message box. The list's
// clientHeight is what the box leaves; its scrollHeight is its content; the
// textarea's scrollHeight is its lines; scrollTop clamps like a browser's
// and a clamp queues a scroll event. Reading the textarea's scrollHeight is
// a forced layout, so a clamp happens there too (with whatever height the
// box has at that moment). ResizeObserver is a fake that fires when the
// modelled sizes change, as the browser's would after layout.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow, ZenThreadView } from '@/lib/zen/zen-data'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { ZenConversation } from './ZenConversationWidget'
import { __resetZenDraftsForTests } from './ZenCompose'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

// ── The modelled layout ─────────────────────────────────────────────────────
const COLUMN = 600 // header excluded: list + message box
const LINE = 24 // one textarea line
const TA_PAD = 12 // textarea padding (6 + 6)
const ROW_PAD = 12 // the rounded row around the textarea (6 + 6)
const BOX_PAD = 24 // the message box's own padding
const MESSAGE = 60
const DOTS = 34
const OLDER_BUTTON = 30
const LIST_PAD = 22

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  fresh = true
  private readonly cb: ResizeObserverCallback
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb
    FakeResizeObserver.instances.push(this)
  }
  observe(el: Element): void {
    if (!this.observed.includes(el)) this.observed.push(el)
    this.fresh = true
  }
  unobserve(el: Element): void {
    this.observed = this.observed.filter((o) => o !== el)
  }
  disconnect(): void {
    this.observed = []
    FakeResizeObserver.instances = FakeResizeObserver.instances.filter((o) => o !== this)
  }
  fire(): void {
    this.fresh = false
    if (this.observed.length === 0) return
    this.cb(
      this.observed.map((target) => ({ target, contentRect: { width: 0, height: 0 } }) as unknown as ResizeObserverEntry),
      this as unknown as ResizeObserver,
    )
  }
}

interface Layout {
  list: HTMLElement
  textarea: HTMLTextAreaElement
  /** The list's scrollTop as the browser holds it. */
  top: number
  /** A clamp happened: the browser will fire a scroll event. */
  scrollQueued: boolean
  sig: string
}

let layout: Layout | null = null

function lay(): Layout {
  if (!layout) throw new Error('layout not installed')
  return layout
}

function textareaHeight(ta: HTMLTextAreaElement): number {
  const h = ta.style.height
  if (h === '' || h === 'auto') return LINE + TA_PAD // rows=1
  const px = Number.parseFloat(h)
  if (!Number.isFinite(px)) throw new Error(`unexpected textarea height ${h}`)
  return px
}

function rowHeight(ta: HTMLTextAreaElement): number {
  const row = ta.parentElement
  if (!row) throw new Error('textarea has no row')
  const own = textareaHeight(ta) + ROW_PAD
  const min = row.style.minHeight ? Number.parseFloat(row.style.minHeight) : 0
  return Math.max(own, min)
}

const composeHeight = (ta: HTMLTextAreaElement): number => rowHeight(ta) + BOX_PAD
const clientHeight = (l: Layout): number => COLUMN - composeHeight(l.textarea)

function contentHeight(list: HTMLElement): number {
  const messages = list.querySelectorAll('[data-zen-message]').length
  const dots = list.querySelector('[data-zen-typing]') ? DOTS : 0
  const older = list.querySelector('[data-zen-load-older]') ? OLDER_BUTTON : 0
  return LIST_PAD + older + messages * MESSAGE + dots
}

const scrollHeight = (l: Layout): number => Math.max(clientHeight(l), contentHeight(l.list))
const maxTop = (l: Layout): number => Math.max(0, scrollHeight(l) - clientHeight(l))

/** The browser's layout pass: clamp scrollTop to what the box allows now. */
function clamp(l: Layout): void {
  const m = maxTop(l)
  if (l.top > m) {
    l.top = m
    l.scrollQueued = true
  }
}

function installLayout(): Layout {
  const list = document.querySelector('[data-zen-conversation-body]')
  const textarea = document.querySelector('[data-zen-compose-input]')
  if (!(list instanceof HTMLElement)) throw new Error('no conversation list')
  if (!(textarea instanceof HTMLTextAreaElement)) throw new Error('no message box')
  const l: Layout = { list, textarea, top: 0, scrollQueued: false, sig: '' }
  layout = l
  Object.defineProperty(list, 'clientHeight', { configurable: true, get: () => clientHeight(l) })
  Object.defineProperty(list, 'scrollHeight', { configurable: true, get: () => scrollHeight(l) })
  Object.defineProperty(list, 'scrollTop', {
    configurable: true,
    get: () => l.top,
    set: (v: number) => {
      l.top = Math.max(0, Math.min(v, maxTop(l)))
    },
  })
  // Reading a textarea's scrollHeight forces layout: the list clamps NOW,
  // with the box at whatever height it has at this instant.
  Object.defineProperty(textarea, 'scrollHeight', {
    configurable: true,
    get: () => {
      clamp(l)
      return textarea.value.split('\n').length * LINE + TA_PAD
    },
  })
  const row = textarea.parentElement
  if (!row) throw new Error('textarea has no row')
  Object.defineProperty(row, 'offsetHeight', { configurable: true, get: () => rowHeight(textarea) })
  return l
}

/** End of a frame: layout clamps, queued scroll events fire, then resize
 *  observers fire when a modelled size changed (or a target is new). */
function frame(): void {
  const l = lay()
  act(() => {
    clamp(l)
    if (l.scrollQueued) {
      l.scrollQueued = false
      fireEvent.scroll(l.list)
    }
  })
  const sig = `${clientHeight(l)}|${contentHeight(l.list)}|${composeHeight(l.textarea)}`
  const changed = sig !== l.sig
  l.sig = sig
  act(() => {
    for (const ro of [...FakeResizeObserver.instances]) if (changed || ro.fresh) ro.fire()
  })
  act(() => {
    clamp(l)
    if (l.scrollQueued) {
      l.scrollQueued = false
      fireEvent.scroll(l.list)
    }
  })
}

/** The person scrolls the list to `top`. */
function userScroll(top: number): void {
  const l = lay()
  act(() => {
    l.list.scrollTop = top
    fireEvent.scroll(l.list)
  })
  frame()
}

function type(text: string): void {
  act(() => {
    fireEvent.change(lay().textarea, { target: { value: text } })
  })
  frame()
}

const atTrueBottom = (): boolean => lay().top === maxTop(lay())

/** The working dots (last in the list) are fully inside the list's box. */
function dotsVisible(): boolean {
  const l = lay()
  if (!l.list.querySelector('[data-zen-typing]')) throw new Error('no working dots to check')
  return l.top + clientHeight(l) >= scrollHeight(l)
}

// ── A fake bridge ───────────────────────────────────────────────────────────
interface FakeBridge {
  bridge: ZenWidgetBridge
  push(view: ZenThreadView): void
  posts: string[]
  reads: unknown[]
}

function fakeBridge(): FakeBridge {
  let listener: ((v: ZenThreadView) => void) | null = null
  const posts: string[] = []
  const reads: unknown[] = []
  const bridge = {
    widgetId: 'conversation',
    caps: new Set<string>(['thread:read', 'thread:post']),
    call(verb: string, ...args: unknown[]): unknown {
      if (verb === 'thread.subscribe') {
        listener = args[1] as (v: ZenThreadView) => void
        return () => {
          listener = null
        }
      }
      if (verb === 'thread.post') {
        posts.push(String(args[1]))
        return Promise.resolve()
      }
      if (verb === 'thread.read') {
        reads.push(args[1])
        return Promise.resolve()
      }
      throw new Error(`unexpected bridge verb ${verb}`)
    },
    gardens: {
      list: () => [],
      current: () => null,
      switch: () => {},
      create: () => Promise.reject(new Error('no')),
    },
    homes: { list: () => [] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return {
    bridge,
    posts,
    reads,
    push(view) {
      if (!listener) throw new Error('nobody subscribed to the thread')
      const fn = listener
      act(() => fn(view))
    },
  }
}

function item(seq: number, mine = false): OverlayThreadItem {
  const id = `m${seq}`
  return {
    collection: 'thread',
    seq,
    id,
    doc: { id, kind: 'text', from: mine ? 'me' : 'cortana', body: `message ${seq}`, created_at: 1, ...(mine ? { via: 'compose' } : {}) },
  } as OverlayThreadItem
}

function view(items: OverlayThreadItem[], over: Partial<ZenThreadView> = {}): ZenThreadView {
  return {
    address: 'cortana::local',
    phase: 'ready',
    note: null,
    items,
    loaded: true,
    hasMore: false,
    loadingOlder: false,
    error: null,
    ...over,
  }
}

function row(working: boolean): ZenAgentRow {
  return {
    address: 'cortana::local',
    label: 'cortana',
    index: 0,
    hostKey: 'local',
    server: null,
    role: null,
    reach: 'live',
    auth: null,
    activity: working ? 'working' : 'idle',
    working,
    needsYou: false,
    state: 'ok',
    stateLabel: null,
    detail: null,
    preview: null,
    people: [],
    selected: true,
    openable: true,
  } as ZenAgentRow
}

const many = (n: number, from = 1): OverlayThreadItem[] => Array.from({ length: n }, (_, i) => item(from + i))

function mount(working: boolean, items: OverlayThreadItem[], over: Partial<ZenThreadView> = {}) {
  const fb = fakeBridge()
  const r = render(<ZenConversation bridge={fb.bridge} row={row(working)} />)
  installLayout()
  frame()
  fb.push(view(items, over))
  frame()
  return { ...fb, rerender: (w: boolean) => r.rerender(<ZenConversation bridge={fb.bridge} row={row(w)} />) }
}

beforeEach(() => {
  FakeResizeObserver.instances = []
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver
  __resetZenDraftsForTests()
})

afterEach(() => {
  cleanup()
  layout = null
  delete (globalThis as { ResizeObserver?: unknown }).ResizeObserver
})

describe('Zen conversation: stick to the bottom', () => {
  it('opens at the true bottom, working dots in view', () => {
    mount(true, many(20))
    expect(maxTop(lay())).toBeGreaterThan(0)
    expect(atTrueBottom()).toBe(true)
    expect(dotsVisible()).toBe(true)
  })

  it('Bug A: typing while at the bottom keeps the working dots in view', () => {
    mount(true, many(20))
    expect(dotsVisible()).toBe(true)
    type('h')
    expect(dotsVisible()).toBe(true)
    type('hello\nsecond line')
    expect(composeHeight(lay().textarea)).toBe(2 * LINE + TA_PAD + ROW_PAD + BOX_PAD)
    expect(dotsVisible()).toBe(true)
    // Another key on a box that is already two lines tall (the measuring
    // pass must not drop the list's scroll position under the box).
    type('hello\nsecond line!')
    expect(dotsVisible()).toBe(true)
    expect(atTrueBottom()).toBe(true)
    // And the dots are still there for the next message too.
    type('hello\nsecond line!\nthird')
    expect(dotsVisible()).toBe(true)
  })

  it('Bug B: the growing box keeps the list pinned while at the bottom; new messages and a shrink too', () => {
    const t = mount(false, many(20))
    expect(atTrueBottom()).toBe(true)
    const boxFor = (lines: number): number => COLUMN - (lines * LINE + TA_PAD + ROW_PAD + BOX_PAD)
    type('1\n2\n3\n4\n5')
    expect(clientHeight(lay())).toBe(boxFor(5))
    expect(atTrueBottom()).toBe(true)
    // A message arrives while pinned: still pinned.
    t.push(view(many(21)))
    frame()
    expect(atTrueBottom()).toBe(true)
    // The agent starts working: the dots are the bottom.
    t.rerender(true)
    frame()
    expect(dotsVisible()).toBe(true)
    // Esc clears: the box shrinks back, still pinned.
    act(() => {
      fireEvent.keyDown(lay().textarea, { key: 'Escape' })
    })
    frame()
    expect(clientHeight(lay())).toBe(boxFor(1))
    expect(dotsVisible()).toBe(true)
  })

  it('scrolled up: typing, the box growing or shrinking, and new messages never move the list', () => {
    const t = mount(true, many(20))
    userScroll(300)
    expect(lay().top).toBe(300)
    type('1\n2\n3\n4\n5')
    expect(lay().top).toBe(300)
    type('1\n2\n3\n4\n5\n6')
    expect(lay().top).toBe(300)
    t.push(view(many(22)))
    frame()
    expect(lay().top).toBe(300)
    t.rerender(false)
    frame()
    expect(lay().top).toBe(300)
    type('')
    expect(lay().top).toBe(300)
  })

  it('a few pixels above the bottom still counts as the bottom', () => {
    mount(true, many(20))
    userScroll(maxTop(lay()) - 20)
    type('1\n2\n3')
    expect(dotsVisible()).toBe(true)
  })

  it('sending scrolls to the bottom, and the sent message lands in view', async () => {
    const t = mount(false, many(20))
    userScroll(200)
    type('ship it')
    await act(async () => {
      fireEvent.keyDown(lay().textarea, { key: 'Enter' })
    })
    frame()
    expect(t.posts).toEqual(['ship it'])
    t.push(view([...many(20), item(21, true)]))
    frame()
    expect(atTrueBottom()).toBe(true)
  })

  it('older messages loading on top keep the reading position', () => {
    const t = mount(false, many(20, 11), { hasMore: true })
    userScroll(0)
    expect(t.reads).toEqual([{ beforeSeq: 11 }])
    t.push(view(many(20, 11), { hasMore: true, loadingOlder: true }))
    frame()
    expect(lay().top).toBe(0)
    t.push(view(many(30, 1), { hasMore: false }))
    frame()
    // Ten messages and the "Earlier messages" button: the same message is
    // at the top of the box as before.
    expect(lay().top).toBe(10 * MESSAGE - OLDER_BUTTON)
  })
})
