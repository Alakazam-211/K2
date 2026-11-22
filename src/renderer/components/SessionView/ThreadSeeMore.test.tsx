// @vitest-environment jsdom
// "See more" in the desktop Thread (MessageFold in ThreadBubble, the fold
// host in ThreadOverlayPane): a long message folds, yours and the agent's;
// only the words fold, never a reply's quote strip, files or a card; Copy
// text takes the full text; a code block folds by its height; a search hit
// opens it; opening the newest while following keeps the reader's place;
// a reply streaming past the threshold folds without the clip moving.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import { ThreadFloatSearch } from './ThreadFloatSearch'
import type { OverlayThreadItem, ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { useContextMenuStore } from '@/stores/context-menu'
import { __resetFoldForTests, foldClipPx } from '@/components/Thread/MessageFold'

const clip = vi.hoisted(() => ({ writeText: vi.fn(async (_t: string) => {}) }))
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: clip.writeText }))

const threadHook = vi.hoisted(() => ({
  items: [] as OverlayThreadItem[],
  conversationId: 'c',
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loaded: true,
  loadOlder: async () => {},
  loadAll: undefined as undefined | (() => Promise<void>),
  turn: null as ThreadTurn | null,
  turnsReported: true,
  outbox: [] as OverlayThreadItem[],
}))
vi.mock('./useOverlayThread', () => ({ useOverlayThread: () => threadHook }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

const room = testRoom({ tabs: {}, scope: primaryScope() })
const LINE = 21

/** 30 short lines: long by height. */
const LONG = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join('\n\n')

function textItem(id: string, seq: number, body: string, doc: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem {
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'sales', body, created_at: 1_700_000_000 + seq, ...doc } }
}

// ── ResizeObserver: each instance with what it observes.
type Obs = { cb: (entries?: ResizeObserverEntry[]) => void; targets: Element[] }
let observers: Obs[] = []
function report(target: Element, h: number): void {
  const o = observers.find((x) => x.targets.includes(target))
  if (!o) throw new Error('not observed')
  act(() => o.cb([{ target, contentRect: { height: h } } as unknown as ResizeObserverEntry]))
}
/** The pane's own observer (list, pane, content): late layout. */
function paneResize(): void {
  const o = observers.find((x) => x.targets.includes(list()))
  act(() => o!.cb([]))
}
const realGcs = window.getComputedStyle.bind(window)

/** A pane as Thread view lays it out: the floating header, then the body. */
function Pane({ children }: { children?: ReactNode }): React.JSX.Element {
  return (
    <TabVisibilityContext.Provider value={true}>
      <div data-testid="host" data-thread-floats-host="">
        <div data-thread-floats="">
          <div className="thread-float-group">
            <ThreadFloatSearch />
          </div>
        </div>
        {children}
      </div>
    </TabVisibilityContext.Provider>
  )
}

function renderPane(): void {
  renderInRoom(room, (
    <Pane>
      <ThreadOverlayPane addr="sales" conversationId="c" />
    </Pane>
  ))
}

const list = (): HTMLElement => screen.getByTestId('thread-overlay-list')
const row = (id: string): HTMLElement => document.querySelector<HTMLElement>(`[data-zen-message="${id}"]`)!
const innerOf = (id: string): HTMLElement => row(id).querySelector<HTMLElement>('[data-testid=fold-inner]')!
const toggleOf = (id: string): HTMLButtonElement | null => row(id).querySelector<HTMLButtonElement>('[data-testid=fold-toggle]')
const clipOf = (id: string): HTMLElement => row(id).querySelector<HTMLElement>('[data-testid=fold-clip]')!

beforeEach(() => {
  __resetFoldForTests()
  observers = []
  clip.writeText.mockClear()
  useContextMenuStore.getState().close()
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  vi.stubGlobal(
    'ResizeObserver',
    class {
      o: Obs
      constructor(cb: Obs['cb']) {
        this.o = { cb, targets: [] }
        observers.push(this.o)
      }
      observe(t: Element): void {
        this.o.targets.push(t)
      }
      unobserve(): void {}
      disconnect(): void {
        this.o.targets = []
      }
    },
  )
  vi.spyOn(window, 'getComputedStyle').mockImplementation((el: Element) => {
    const cs = realGcs(el)
    if (!el.classList?.contains('chat-markdown')) return cs
    return new Proxy(cs, {
      get(t, k: string) {
        if (k === 'lineHeight') return `${LINE}px`
        if (k === 'fontSize') return '13px'
        const v = Reflect.get(t, k)
        return typeof v === 'function' ? v.bind(t) : v
      },
    })
  })
  threadHook.items = [textItem('m1', 41, 'Short one.'), textItem('m2', 42, LONG)]
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  resetThreadViewersForTests()
  threadHook.items = []
  threadHook.outbox = []
})

describe('See more in the desktop Thread', () => {
  it('folds only the long message, clipped at 20 lines (19 and the pill’s row); the words stay in the DOM', () => {
    renderPane()
    report(innerOf('m1'), LINE)
    report(innerOf('m2'), LINE * 60)
    expect(toggleOf('m1')).toBeNull()
    const t = toggleOf('m2')!
    expect(t.textContent).toBe('See more')
    expect(t.getAttribute('aria-expanded')).toBe('false')
    expect(t.getAttribute('aria-controls')).toBe(clipOf('m2').id)
    expect(t.querySelector('[data-testid=fold-chevron]')!.getAttribute('data-dir')).toBe('down')
    expect(clipOf('m2').style.maxHeight).toBe(`${foldClipPx(LINE)}px`)
    // Screen readers read them; selection works.
    expect(row('m2').textContent).toContain('line 30')
  })

  it('19 lines stay whole; 21 lines fold', () => {
    renderPane()
    report(innerOf('m2'), LINE * 19)
    expect(toggleOf('m2')).toBeNull()
    report(innerOf('m2'), LINE * 21)
    expect(toggleOf('m2')!.textContent).toBe('See more')
  })

  it('your own long message folds too, in your bubble', () => {
    threadHook.items = [textItem('m3', 43, LONG, { mine: true, from: 'owner' })]
    renderPane()
    report(innerOf('m3'), LINE * 60)
    expect(row('m3').querySelector('[data-zen-bubble]')!.getAttribute('data-zen-bubble')).toBe('me')
    expect(toggleOf('m3')!.textContent).toBe('See more')
  })

  it('a click opens it in place: See less, caret up, no message menu', () => {
    renderPane()
    report(innerOf('m2'), LINE * 60)
    fireEvent.click(toggleOf('m2')!)
    expect(toggleOf('m2')!.textContent).toBe('See less')
    expect(toggleOf('m2')!.getAttribute('aria-expanded')).toBe('true')
    expect(toggleOf('m2')!.querySelector('[data-testid=fold-chevron]')!.getAttribute('data-dir')).toBe('up')
    expect(clipOf('m2').style.maxHeight).toBe('')
    expect(useContextMenuStore.getState().isOpen).toBe(false)
  })

  it('Copy text takes the full text while folded', async () => {
    renderPane()
    report(innerOf('m2'), LINE * 60)
    fireEvent.contextMenu(row('m2').querySelector('[data-zen-bubble]')!, { clientX: 10, clientY: 10 })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    act(() => useContextMenuStore.getState().selectItem('copy'))
    await waitFor(() => expect(clip.writeText).toHaveBeenCalledWith(LONG))
    expect(toggleOf('m2')!.getAttribute('aria-expanded')).toBe('false')
  })

  it('a long fenced code block folds by its height, not its characters', () => {
    const code = '```\n' + Array.from({ length: 40 }, (_, i) => `step_${i}()`).join('\n') + '\n```'
    const prose = 'word '.repeat(600)
    threadHook.items = [textItem('c1', 1, code), textItem('p1', 2, prose)]
    renderPane()
    report(innerOf('c1'), LINE * 40)
    // Thousands of characters that wrap to only a few lines in a wide column.
    report(innerOf('p1'), LINE * 6)
    expect(toggleOf('c1')!.textContent).toBe('See more')
    expect(clipOf('c1').querySelector('pre')).not.toBeNull()
    expect(toggleOf('p1')).toBeNull()
  })

  it('only the words fold: a reply’s quote strip, the files under it and a card stay whole', () => {
    const file = { kind: 'file', path: 'notes/x.txt', name: 'x.txt', size: 1234, mime: 'text/plain', modifiedAt: 1_760_000_000_000, sha256: 'ab'.repeat(32), state: 'ok' }
    threadHook.items = [
      textItem('r1', 1, `↩ #41 sales:\n> Short one.\n\n${LONG}`, { attachments: [file] as never }),
      {
        collection: 'thread',
        seq: 2,
        id: 'q1',
        doc: {
          id: 'q1',
          kind: 'choice',
          from: 'sales',
          created_at: 1,
          body: LONG,
          choice: { options: ['Ship it', 'Wait'], allow_custom: false, status: 'waiting' },
        },
      } as unknown as OverlayThreadItem,
    ]
    renderPane()
    report(innerOf('r1'), LINE * 60)
    expect(toggleOf('r1')!.textContent).toBe('See more')
    const strip = row('r1').querySelector('[data-thread-reply-quote]')!
    expect(strip).not.toBeNull()
    expect(clipOf('r1').contains(strip)).toBe(false)
    const fileName = row('r1').querySelector('[data-thread-file-name]')!
    expect(fileName.textContent).toContain('x.txt')
    expect(clipOf('r1').contains(fileName)).toBe(false)
    // A card's question never folds; its options stay in view.
    expect(row('q1').querySelector('[data-testid=fold-clip]')).toBeNull()
    expect(row('q1').querySelector('[data-testid=thread-choice-chip]')).not.toBeNull()
  })

  it('opening the newest message while following keeps the reader’s place; folding back at the bottom follows again', () => {
    renderPane()
    const box = { scrollHeight: 1800, clientHeight: 600, scrollTop: 0 }
    const el = list()
    Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => box.scrollHeight })
    Object.defineProperty(el, 'clientHeight', { configurable: true, get: () => box.clientHeight })
    Object.defineProperty(el, 'scrollTop', {
      configurable: true,
      get: () => box.scrollTop,
      set: (v: number) => {
        box.scrollTop = Math.max(0, Math.min(v, box.scrollHeight - box.clientHeight))
      },
    })
    paneResize()
    expect(box.scrollTop).toBe(1200)
    report(innerOf('m2'), LINE * 60)
    fireEvent.click(toggleOf('m2')!)
    // The open message lays out taller: late layout must not stick.
    box.scrollHeight = 2600
    paneResize()
    expect(box.scrollTop).toBe(1200)
    // Folding back at the bottom follows again.
    box.scrollHeight = 1800
    fireEvent.click(toggleOf('m2')!)
    box.scrollHeight = 1900
    paneResize()
    expect(box.scrollTop).toBe(1300)
  })

  it('an agent reply streaming past the threshold folds; the clip height never changes', () => {
    const { rerender } = renderInRoom(room, (
      <Pane>
        <ThreadOverlayPane addr="sales" conversationId="c" />
      </Pane>
    ))
    const grow = (lines: number): void => {
      threadHook.items = [
        textItem('m1', 41, 'Short one.'),
        textItem('m3', 43, Array.from({ length: lines }, (_, i) => `step ${i}`).join('\n\n')),
      ]
      act(() =>
        rerender(
          <Pane>
            <ThreadOverlayPane addr="sales" conversationId="c" agentName={`k2 ${lines}`} />
          </Pane>,
        ),
      )
      report(innerOf('m3'), LINE * lines)
    }
    grow(4)
    expect(toggleOf('m3')).toBeNull()
    expect(clipOf('m3').style.maxHeight).toBe(`${foldClipPx(LINE)}px`)
    grow(19)
    expect(toggleOf('m3')).toBeNull()
    grow(24)
    expect(toggleOf('m3')!.textContent).toBe('See more')
    expect(clipOf('m3').style.maxHeight).toBe(`${foldClipPx(LINE)}px`)
  })
})

describe('a search hit in a folded message', () => {
  const g = globalThis as unknown as { CSS?: unknown; Highlight?: unknown }
  let prev: { css: unknown; h: unknown }
  beforeEach(() => {
    prev = { css: g.CSS, h: g.Highlight }
    g.CSS = { highlights: new Map() }
    g.Highlight = class {
      ranges: Range[]
      constructor(...r: Range[]) {
        this.ranges = r
      }
    }
  })
  afterEach(() => {
    g.CSS = prev.css
    g.Highlight = prev.h
  })
  const flush = async (): Promise<void> => {
    await act(async () => {
      await new Promise((r) => requestAnimationFrame(() => r(null)))
    })
  }

  it('opens the fold, so the match is in view', async () => {
    threadHook.items = [textItem('m1', 41, 'Short one.'), textItem('m2', 42, `${LONG}\n\nthe zebra at the end`)]
    renderPane()
    report(innerOf('m1'), LINE)
    report(innerOf('m2'), LINE * 60)
    expect(toggleOf('m2')!.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(screen.getByTestId('thread-search-button'))
    fireEvent.change(screen.getByLabelText('Search messages in this Thread'), { target: { value: 'zebra' } })
    await flush()
    await flush()
    expect(screen.getByTestId('thread-search-count').textContent).toContain('1 of 1')
    expect(toggleOf('m2')!.getAttribute('aria-expanded')).toBe('true')
    expect(clipOf('m2').style.maxHeight).toBe('')
  })

  it('a match in a short message leaves the long one folded', async () => {
    threadHook.items = [textItem('m1', 41, 'Short zebra.'), textItem('m2', 42, LONG)]
    renderPane()
    report(innerOf('m1'), LINE)
    report(innerOf('m2'), LINE * 60)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    fireEvent.change(screen.getByLabelText('Search messages in this Thread'), { target: { value: 'zebra' } })
    await flush()
    await flush()
    expect(toggleOf('m2')!.getAttribute('aria-expanded')).toBe('false')
  })
})
