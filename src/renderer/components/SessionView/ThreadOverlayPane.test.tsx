// @vitest-environment jsdom
import { describe, expect, it, afterEach, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, act } from '@testing-library/react'
import { createElement, useState, type ReactNode } from 'react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import {
  choiceLetter,
  ThreadItemRow,
  ThreadOverlayPane,
} from './ThreadOverlayPane'
import { overlayViewer } from './sessionViewTab'
import type { OverlayThreadItem } from './overlayThread'

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
  loadOlder: async () => {},
}))

vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) =>
    sel({ editor: { fontSize: 13 } }),
}))

describe('Thread overlay pane', () => {
  afterEach(() => {
    cleanup()
    threadHook.items = []
    threadHook.error = null
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
  })

  it('has no compose box — Message-the-agent stays on the terminal bar', () => {
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
  })

  it('overlay root is a flex-1 min-h-0 overflow-hidden column (not height 100%)', () => {
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    const pane = screen.getByTestId('thread-overlay-pane')
    expect(pane.className).toContain('flex-1')
    expect(pane.className).toContain('min-h-0')
    expect(pane.className).toContain('overflow-hidden')
    expect(pane.className.split(/\s+/)).not.toContain('h-full')
  })

  it('shows Load older when hasMore and click calls loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    const list = overlayList()
    stubListBox(list, { scrollHeight: 800, clientHeight: 200, scrollTop: 40 })
    fireEvent.click(screen.getByTestId('overlay-load-older'))
    expect(loadOlder).toHaveBeenCalledTimes(1)
  })

  it('hides Load older when hasMore is false', () => {
    render(<ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.queryByTestId('overlay-load-older')).toBeNull()
  })
})

describe('Thread overlay choice chips + secret field', () => {
  afterEach(() => cleanup())

  it('letters options A, B, … then AA', () => {
    expect(choiceLetter(0)).toBe('A')
    expect(choiceLetter(1)).toBe('B')
    expect(choiceLetter(25)).toBe('Z')
    expect(choiceLetter(26)).toBe('AA')
  })

  it('renders markdown in a project-chat style message', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 't1',
      doc: {
        id: 't1',
        kind: 'text',
        from: 'owner',
        via: 'compose',
        created_at: Math.floor(Date.now() / 1000),
        body: '**Hello** and `code`',
      },
    }
    render(<ThreadItemRow item={item} />)
    expect(screen.getByText('You')).not.toBeNull()
    expect(screen.getByText('Hello').tagName).toBe('STRONG')
    expect(screen.getByText('code').tagName).toBe('CODE')
  })

  it('renders a vertical lettered choice card; first option is primary; tap calls onAnswer', () => {
    const picks: string[] = []
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'c1',
      doc: {
        id: 'c1',
        kind: 'choice',
        from: 'k2',
        body: 'Ship it?',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'pending',
        },
      },
    }
    render(<ThreadItemRow item={item} onAnswer={(p) => picks.push(p.answer || '')} />)
    const card = screen.getByTestId('thread-choice-card')
    expect(card.className).toContain('flex-col')
    expect(card.className).toContain('w-full')
    expect(card.className).not.toContain('max-w-sm')
    expect(card.className).toMatch(/\bpx-2\b/)
    const chips = screen.getAllByTestId('thread-choice-chip')
    expect(chips).toHaveLength(2)
    expect(chips[0].getAttribute('data-letter')).toBe('A')
    expect(chips[1].getAttribute('data-letter')).toBe('B')
    expect(chips[0].textContent).toContain('A')
    expect(chips[0].textContent).toContain('Go')
    expect(chips[0].getAttribute('data-primary')).toBe('true')
    expect(chips[1].getAttribute('data-primary')).toBe('false')
    fireEvent.click(chips[0])
    expect(picks).toEqual(['Go'])
  })

  it('answered choice stays visible with the selected option highlighted', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'c1',
      doc: {
        id: 'c1',
        kind: 'choice',
        from: 'k2',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'answered',
          answer: 'Go',
        },
      },
    }
    render(<ThreadItemRow item={item} />)
    expect(screen.getByTestId('thread-choice-card')).not.toBeNull()
    const go = screen.getAllByTestId('thread-choice-chip')[0]
    expect(go.getAttribute('disabled')).not.toBeNull()
    expect(go.getAttribute('data-letter')).toBe('A')
    expect(go.className).toContain('border-[var(--color-accent)]')
    expect(go.className).toContain('bg-[var(--color-accent)]/15')
  })

  it('renders a secret field and submit/dismiss; never shows the value as body text', () => {
    const submitted: string[] = []
    let dismissed = 0
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 2,
      id: 's1',
      doc: {
        id: 's1',
        kind: 'secret',
        from: 'k2',
        secret: { name: 'API_TOKEN', status: 'pending', prompt: 'Paste the Grok token' },
      },
    }
    render(
      <ThreadItemRow
        item={item}
        onAnswer={(p) => submitted.push(p.secret || '')}
        onVoid={() => {
          dismissed += 1
        }}
      />,
    )
    const field = screen.getByTestId('thread-secret-field') as HTMLInputElement
    expect(field.type).toBe('password')
    fireEvent.change(field, { target: { value: 's3cr3t-bytes' } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect(submitted).toEqual(['s3cr3t-bytes'])
    expect(screen.queryByText('s3cr3t-bytes')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-secret-dismiss'))
    expect(dismissed).toBe(1)
  })
})

type ListBox = {
  scrollHeight: number
  clientHeight: number
  scrollTop: number
}

function textItem(id: string, seq: number): OverlayThreadItem {
  return {
    collection: 'thread',
    seq,
    id,
    doc: {
      id,
      kind: 'text',
      from: 'k2',
      body: `m${seq}`,
      created_at: 1,
    },
  }
}

function overlayList(): HTMLElement {
  const pane = screen.getByTestId('thread-overlay-pane')
  const list =
    pane.querySelector('[data-testid="thread-overlay-list"]') ??
    pane.querySelector('.overflow-y-auto')
  if (!(list instanceof HTMLElement)) throw new Error('missing thread overlay list')
  return list
}

function stubListBox(el: HTMLElement, box: ListBox): void {
  Object.defineProperty(el, 'scrollHeight', {
    configurable: true,
    get: () => box.scrollHeight,
  })
  Object.defineProperty(el, 'clientHeight', {
    configurable: true,
    get: () => box.clientHeight,
  })
  Object.defineProperty(el, 'scrollTop', {
    configurable: true,
    get: () => box.scrollTop,
    set: (v: number) => {
      box.scrollTop = v
    },
  })
}

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  private readonly cb: ResizeObserverCallback
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb
    FakeResizeObserver.instances.push(this)
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
    this.cb(
      this.observed.map(
        (target) =>
          ({
            target,
            contentRect: { width: 0, height: 0 },
          }) as ResizeObserverEntry,
      ),
      this as unknown as ResizeObserver,
    )
  }
}

function observerOf(el: Element): FakeResizeObserver {
  const found = FakeResizeObserver.instances.find((o) => o.observed.includes(el))
  if (!found) throw new Error('no ResizeObserver is observing the list')
  return found
}

function renderOverlay(visible = true) {
  return render(
    createElement(
      TabVisibilityContext.Provider,
      { value: visible },
      createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
    ),
  )
}

function VisibilityHarness({
  visible,
  children,
}: {
  visible: boolean
  children: ReactNode
}) {
  return createElement(TabVisibilityContext.Provider, { value: visible }, children)
}

describe('Thread overlay list scroll restore', () => {
  beforeEach(() => {
    FakeResizeObserver.instances = []
    vi.stubGlobal('ResizeObserver', FakeResizeObserver)
    threadHook.items = [textItem('t1', 1), textItem('t2', 2)]
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
  })

  afterEach(() => {
    cleanup()
    threadHook.items = []
    threadHook.error = null
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
    vi.unstubAllGlobals()
  })

  it('(a) items + real height pin to bottom', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(b) 0-height then ResizeObserver to real height pins if pinned', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 0, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(0)

    box.clientHeight = 200
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(c) user scrolled up stays put on resize and new items', () => {
    const view = renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    box.scrollTop = 120
    fireEvent.scroll(list)
    expect(box.scrollTop).toBe(120)

    box.clientHeight = 140
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(120)

    threadHook.items = [...threadHook.items, textItem('t3', 3)]
    view.rerender(
      createElement(
        TabVisibilityContext.Provider,
        { value: true },
        createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
      ),
    )
    expect(box.scrollTop).toBe(120)
  })

  it('(d) scrollTop === 0 at clientHeight === 0 does not call loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 0, scrollTop: 0 }
    stubListBox(list, box)
    fireEvent.scroll(list)
    expect(loadOlder).not.toHaveBeenCalled()
  })

  it('(e) split and thread-only both overlayViewer.thread', () => {
    expect(overlayViewer('thread').thread).toBe(true)
    expect(overlayViewer('split').thread).toBe(true)
    expect(overlayViewer('terminal').thread).toBe(false)
    expect(overlayViewer('chatter').thread).toBe(false)
  })

  it('(f) list clientHeight shrinks while pinned stays at bottom', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    box.clientHeight = 80
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(f2) compose-grow scroll event before ResizeObserver does not unpin', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    // Browser keeps old scrollTop; new clientHeight is no longer at bottom.
    box.clientHeight = 80
    box.scrollTop = 600
    fireEvent.scroll(list)
    expect(box.scrollTop).toBe(box.scrollHeight)

    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(g) 0-height onScroll does not clear pinBottomRef', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    box.clientHeight = 0
    box.scrollTop = 0
    fireEvent.scroll(list)

    box.clientHeight = 200
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('visibility false→true re-pins when still pinned', () => {
    function Flip() {
      const [visible, setVisible] = useState(false)
      return createElement(
        'div',
        null,
        createElement(VisibilityHarness, {
          visible,
          children: createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
        }),
        createElement('button', {
          type: 'button',
          'data-testid': 'flip-visible',
          onClick: () => setVisible(true),
        }),
      )
    }
    render(createElement(Flip))
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    expect(box.scrollTop).toBe(0)

    fireEvent.click(screen.getByTestId('flip-visible'))
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('visibility false→true does not steal a scrolled-up list', () => {
    function Flip() {
      const [visible, setVisible] = useState(true)
      return createElement(
        'div',
        null,
        createElement(VisibilityHarness, {
          visible,
          children: createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
        }),
        createElement('button', {
          type: 'button',
          'data-testid': 'hide-tab',
          onClick: () => setVisible(false),
        }),
        createElement('button', {
          type: 'button',
          'data-testid': 'show-tab',
          onClick: () => setVisible(true),
        }),
      )
    }
    render(createElement(Flip))
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    box.scrollTop = 90
    fireEvent.scroll(list)

    fireEvent.click(screen.getByTestId('hide-tab'))
    box.scrollTop = 0
    box.clientHeight = 0
    fireEvent.scroll(list)
    box.clientHeight = 200
    fireEvent.click(screen.getByTestId('show-tab'))
    expect(box.scrollTop).toBe(90)
  })

  it('hidden tab onScroll does not call loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    renderOverlay(false)
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    fireEvent.scroll(list)
    expect(loadOlder).not.toHaveBeenCalled()
  })
})
