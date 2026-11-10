// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { ThreadFloatSearch, isThreadFindKey } from './ThreadFloatSearch'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import {
  THREAD_SEARCH_HIGHLIGHT,
  THREAD_SEARCH_HIGHLIGHT_CURRENT,
  findThreadMatches,
  threadSearchState,
  updateThreadSearch,
} from './threadSearch'
import type { OverlayThreadItem, ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'

vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => true,
}))

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
  outbox: [] as unknown[],
}))
vi.mock('./useOverlayThread', () => ({ useOverlayThread: () => threadHook }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

const room = testRoom({ tabs: {}, scope: primaryScope() })

/** A pane as Thread view lays it out: the floating header, then the body. */
function Pane({ children }: { children?: ReactNode }): React.JSX.Element {
  return (
    <div data-testid="host" data-thread-floats-host="">
      <div data-thread-floats="">
        <div className="thread-float-group">
          <ThreadFloatSearch />
        </div>
      </div>
      <div data-terminal-container="">
        <textarea data-testid="terminal-input" />
      </div>
      {children}
    </div>
  )
}

const host = (): HTMLElement => screen.getByTestId('host')

function item(i: number, body: string): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: i + 1,
    id: `m${i}`,
    doc: { id: `m${i}`, kind: 'text', from: 'sales', created_at: 1_700_000_000 + i, body },
  } as OverlayThreadItem
}

beforeEach(() => {
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
})
afterEach(() => {
  cleanup()
  resetThreadViewersForTests()
  threadHook.items = []
  threadHook.hasMore = false
  threadHook.loadAll = undefined
})

describe('Thread view’s search object', () => {
  it('is a round floating object with a label and the shortcut', () => {
    render(<Pane />)
    const b = screen.getByTestId('thread-search-button')
    expect(b.getAttribute('data-k2-float')).toBe('round')
    expect(b.getAttribute('data-material')).toBe('frost')
    expect(b.getAttribute('aria-label')).toBe('Search this Thread')
    expect(b.getAttribute('aria-keyshortcuts')).toBe('Meta+F')
  })

  it('opens a labelled field in a pill, focused; says plainly when nothing matches', () => {
    render(<Pane />)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    const input = screen.getByLabelText('Search messages in this Thread')
    expect(document.activeElement).toBe(input)
    expect(screen.getByRole('search').getAttribute('data-k2-float')).toBe('pill')
    fireEvent.change(input, { target: { value: 'zebra' } })
    const status = screen.getByRole('status')
    expect(status.getAttribute('aria-live')).toBe('polite')
    expect(status.textContent).toContain('No matches')
    expect((screen.getByTestId('thread-search-older') as HTMLButtonElement).disabled).toBe(true)
  })

  it('Enter goes to the older match, Shift+Enter the newer; the count follows', () => {
    render(<Pane />)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    const input = screen.getByLabelText('Search messages in this Thread')
    fireEvent.change(input, { target: { value: 'x' } })
    const matches = findThreadMatches([{ id: 'a', text: 'x' }, { id: 'b', text: 'x' }, { id: 'c', text: 'x' }], 'x')
    act(() => updateThreadSearch(host(), { matches }))
    expect(screen.getByTestId('thread-search-count').textContent).toContain('1 of 3')
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(screen.getByTestId('thread-search-count').textContent).toContain('2 of 3')
    expect(threadSearchState(host()).current).toBe(matches[1].key)
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    expect(screen.getByTestId('thread-search-count').textContent).toContain('3 of 3')
    fireEvent.click(screen.getByTestId('thread-search-older'))
    expect(screen.getByTestId('thread-search-count').textContent).toContain('1 of 3')
  })

  it('Esc closes it, clears the search and gives focus back to the button', () => {
    render(<Pane />)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    const input = screen.getByLabelText('Search messages in this Thread')
    fireEvent.change(input, { target: { value: 'x' } })
    fireEvent.keyDown(input, { key: 'Escape' })
    expect(screen.queryByRole('search')).toBeNull()
    expect(threadSearchState(host()).query).toBe('')
    expect(document.activeElement).toBe(screen.getByTestId('thread-search-button'))
  })

  it('a click elsewhere closes it', () => {
    render(<Pane />)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    fireEvent.mouseDown(document.body)
    expect(screen.queryByRole('search')).toBeNull()
  })

  it('⌘F in the Thread opens it; in the terminal the terminal keeps it', () => {
    render(
      <Pane>
        <div data-testid="thread-list" tabIndex={-1} />
      </Pane>,
    )
    const terminal = screen.getByTestId('terminal-input')
    const ev = fireEvent.keyDown(terminal, { key: 'f', metaKey: true })
    expect(ev).toBe(true)
    expect(screen.queryByRole('search')).toBeNull()
    fireEvent.keyDown(screen.getByTestId('thread-list'), { key: 'f', metaKey: true })
    expect(screen.getByRole('search')).not.toBeNull()
  })

  it('short of room, the row makes way for the field while it is open', () => {
    render(<Pane />)
    const header = document.querySelector('[data-thread-floats]') as HTMLElement
    Object.defineProperty(header, 'clientWidth', { value: 300, configurable: true })
    fireEvent.click(screen.getByTestId('thread-search-button'))
    expect(header.getAttribute('data-thread-searching')).toBe('tight')
    Object.defineProperty(header, 'clientWidth', { value: 600, configurable: true })
    Object.defineProperty(header, 'scrollWidth', { value: 700, configurable: true })
    fireEvent.keyDown(screen.getByLabelText('Search messages in this Thread'), { key: 'Escape' })
    expect(header.hasAttribute('data-thread-searching')).toBe(false)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    expect(header.getAttribute('data-thread-searching')).toBe('narrow')
  })

  it('the shortcut is ⌘F on a Mac, Ctrl+F elsewhere, nothing else', () => {
    const k = (o: Partial<KeyboardEvent>) => ({ key: 'f', metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...o })
    expect(isThreadFindKey(k({ metaKey: true }), true)).toBe(true)
    expect(isThreadFindKey(k({ ctrlKey: true }), true)).toBe(false)
    expect(isThreadFindKey(k({ ctrlKey: true }), false)).toBe(true)
    expect(isThreadFindKey(k({ metaKey: true, shiftKey: true }), true)).toBe(false)
    expect(isThreadFindKey(k({ key: 'g', metaKey: true }), true)).toBe(false)
  })
})

describe('with the Thread', () => {
  const g = globalThis as unknown as { CSS?: unknown; Highlight?: unknown }
  let reg: Map<string, { ranges: Range[] }>
  let prev: { css: unknown; h: unknown }
  beforeEach(() => {
    prev = { css: g.CSS, h: g.Highlight }
    reg = new Map()
    g.CSS = { highlights: reg }
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

  it('counts every message it holds, draws the current one and paints the matches', async () => {
    threadHook.items = Array.from({ length: 80 }, (_, i) =>
      item(i, i % 20 === 0 ? `release **notes** for build ${i}` : `chatter ${i}`),
    )
    renderInRoom(room, (
      <Pane>
        <ThreadOverlayPane addr="sales" conversationId="c" />
      </Pane>
    ))
    // Only the newest 30 are drawn; m0 and m20 are not.
    expect(document.querySelector('[data-zen-message="m20"]')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-search-button'))
    fireEvent.change(screen.getByLabelText('Search messages in this Thread'), { target: { value: 'RELEASE' } })
    await flush()
    expect(screen.getByTestId('thread-search-count').textContent).toContain('1 of 4')
    expect(reg.get(THREAD_SEARCH_HIGHLIGHT_CURRENT)?.ranges[0].toString()).toBe('release')
    // Older: m40, then m20, which the window draws to show it.
    fireEvent.keyDown(screen.getByLabelText('Search messages in this Thread'), { key: 'Enter' })
    fireEvent.keyDown(screen.getByLabelText('Search messages in this Thread'), { key: 'Enter' })
    await flush()
    await flush()
    expect(screen.getByTestId('thread-search-count').textContent).toContain('3 of 4')
    const current = reg.get(THREAD_SEARCH_HIGHLIGHT_CURRENT)?.ranges[0]
    expect(current?.startContainer.parentElement?.closest('[data-zen-message]')?.getAttribute('data-zen-message')).toBe('m20')
    // The bold word stays bold: nothing was rewritten.
    expect(document.querySelector('[data-zen-message="m20"] strong')?.textContent).toBe('notes')
    // Esc: nothing stays painted.
    fireEvent.keyDown(screen.getByLabelText('Search messages in this Thread'), { key: 'Escape' })
    await flush()
    expect(reg.size).toBe(0)
  })

  it('reads the rest of the history once a search starts', async () => {
    const loadAll = vi.fn(async () => {})
    threadHook.items = [item(0, 'hello')]
    threadHook.hasMore = true
    threadHook.loadAll = loadAll
    renderInRoom(room, (
      <Pane>
        <ThreadOverlayPane addr="sales" conversationId="c" />
      </Pane>
    ))
    fireEvent.click(screen.getByTestId('thread-search-button'))
    expect(loadAll).not.toHaveBeenCalled()
    fireEvent.change(screen.getByLabelText('Search messages in this Thread'), { target: { value: 'he' } })
    fireEvent.change(screen.getByLabelText('Search messages in this Thread'), { target: { value: 'hel' } })
    await flush()
    expect(loadAll).toHaveBeenCalledTimes(1)
  })
})
