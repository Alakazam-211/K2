// @vitest-environment jsdom
//
// thread-read-v1 (0.46.1) — what the unread surfaces draw, and the
// read-while-looking rule:
//   - a row with unread shows the count ("99+" above 99), muted draws grey,
//     waiting draws the amber (needs-you) marker;
//   - the Thread view menu draws no unread count, dot or waiting marker
//     (main nav only, Rosson 2026-10-21), and keeps a Mute / Unmute
//     Thread item that posts /cli/thread/mute;
//   - useAutoMarkRead: visible + focused with a newer seq posts read ONCE
//     with that seq; not focused / not visible (panel collapsed) posts
//     nothing; the same seq again posts nothing.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/stores/session-events', () => ({
  onReadStateChanged: vi.fn(() => () => undefined),
  onAppHello: vi.fn(() => () => undefined),
}))
const cli = vi.hoisted(() => ({
  get: vi.fn(async (_path: string): Promise<unknown> => ({})),
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string) => cli.get(path)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
  }
})
const menu = vi.hoisted(() => ({
  items: [] as Array<{ id: string; label: string; badge?: string; type?: string }>,
  pick: null as string | null,
}))
vi.mock('@/stores/context-menu', () => ({
  useContextMenuStore: {
    getState: () => ({
      show: vi.fn(async (_x: number, _y: number, items: typeof menu.items) => {
        menu.items = items
        return menu.pick
      }),
    }),
  },
}))

import { act } from 'react'
import { cleanup, fireEvent, render, renderHook, waitFor } from '@testing-library/react'
import { UnreadBadge, WorkspaceThreadUnreadBadge } from './UnreadBadge'
import { SessionViewMenu } from '@/components/SessionView/SessionViewMenu'
import {
  __resetUnreadForTests,
  initUnreadEvents,
  useAutoMarkRead,
  useUnreadStore,
  type UnreadEntry,
} from '@/stores/unread'
import { noteServerVersion } from '@/kessel/server-scope'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function entry(over: Partial<UnreadEntry>): UnreadEntry {
  return {
    kind: 'thread',
    id: 'c1',
    projectId: 'p1',
    lastReadSeq: 3,
    lastSeq: 5,
    unread: 2,
    waiting: 0,
    muted: false,
    mutedUntil: null,
    ...over,
  }
}

function seed(...entries: UnreadEntry[]): void {
  const byKey: Record<string, UnreadEntry> = {}
  for (const e of entries) byKey[`${e.kind}:${e.id}`] = e
  useUnreadStore.setState({ byKey, total: 2, waitingTotal: 0, loaded: true })
}

let focused = true
let hasFocus: ReturnType<typeof vi.spyOn>

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

beforeEach(async () => {
  __resetUnreadForTests()
  noteServerVersion('local', '0.46.1', ['thread-read-v1'])
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockResolvedValue({ ok: true, total: 0, waitingTotal: 0, byConversation: [], muted: [] })
  cli.post.mockImplementation(async (path: string, body?: unknown) => {
    const b = body as { conversationId?: string; group?: string; seq?: number; id?: string; muted?: boolean }
    if (path === 'thread/mute') {
      return { ok: true, changed: true, kind: 'thread', id: b.id, seq: 3, lastReadSeq: 3, lastSeq: 5, unread: 2, waiting: 0, muted: b.muted, total: 0, waitingTotal: 0 }
    }
    const id = b.conversationId ?? b.group
    return { ok: true, changed: true, kind: path === 'thread/read' ? 'thread' : 'project', id, seq: b.seq, lastReadSeq: b.seq, lastSeq: b.seq, unread: 0, waiting: 0, muted: false, total: 0, waitingTotal: 0 }
  })
  initUnreadEvents()
  await flush()
  menu.items = []
  menu.pick = null
  focused = true
  hasFocus = vi.spyOn(document, 'hasFocus').mockImplementation(() => focused)
})

afterEach(() => {
  cleanup()
  hasFocus.mockRestore()
})

describe('UnreadBadge', () => {
  it('shows the count, and 99+ above 99', () => {
    const { container, rerender } = render(<UnreadBadge unread={7} />)
    const count = container.querySelector('[data-unread-count]')
    expect(count?.textContent).toBe('7')
    expect(count?.className).toContain('bg-[var(--color-accent)]')
    expect(count?.getAttribute('data-muted')).toBe('false')
    rerender(<UnreadBadge unread={100} />)
    expect(container.querySelector('[data-unread-count]')?.textContent).toBe('99+')
    rerender(<UnreadBadge unread={99} />)
    expect(container.querySelector('[data-unread-count]')?.textContent).toBe('99')
  })

  it('muted keeps the count, grey', () => {
    const { container } = render(<UnreadBadge unread={3} muted />)
    const count = container.querySelector('[data-unread-count]')
    expect(count?.textContent).toBe('3')
    expect(count?.getAttribute('data-muted')).toBe('true')
    expect(count?.className).toContain('bg-[var(--color-text-muted)]')
    expect(count?.className).not.toContain('bg-[var(--color-accent)]')
  })

  it('waiting draws the amber needs-you marker, apart from unread', () => {
    const { container } = render(<UnreadBadge unread={0} waiting={2} />)
    const waiting = container.querySelector('[data-unread-waiting]')
    expect(waiting?.textContent).toBe('2')
    expect(waiting?.className).toContain('bg-[var(--color-status-needs-you)]')
    expect(container.querySelector('[data-unread-count]')).toBeNull()
  })

  it('nothing unread and nothing waiting draws nothing', () => {
    const { container } = render(<UnreadBadge unread={0} waiting={0} muted />)
    expect(container.innerHTML).toBe('')
  })

  it('the dot variant draws dots, no numbers', () => {
    const { container } = render(<UnreadBadge unread={5} waiting={1} variant="dot" />)
    expect(container.querySelector('[data-unread-count]')?.textContent).toBe('')
    expect(container.querySelector('[data-unread-waiting]')?.textContent).toBe('')
  })
})

describe('Thread view menu', () => {
  function renderMenu(): HTMLElement {
    const { getByTestId } = render(
      <SessionViewMenu
        value="terminal"
        splitLeft="terminal"
        splitRight="thread"
        chatEligible={false}
        onChange={() => {}}
        onSplitLeft={() => {}}
        onSplitRight={() => {}}
        threadConversationId="c1"
      />,
    )
    return getByTestId('session-view-button')
  }

  // Rosson 2026-10-21: the unread count and the amber "waiting on you"
  // marker belong in the main nav (the sidebar agent row draws
  // WorkspaceThreadUnreadBadge), not on the view drop-down or its rows.
  it('unread Thread: the count is in the main nav, not on the view button or its rows', async () => {
    seed(entry({ unread: 120 }))
    const nav = render(<WorkspaceThreadUnreadBadge projectId="p1" />)
    expect(nav.container.querySelector('[data-unread-count]')?.textContent).toBe('99+')

    const button = renderMenu()
    expect(button.querySelector('[data-unread-badge]')).toBeNull()
    expect(button.querySelector('[data-unread-count]')).toBeNull()
    expect(button.textContent).toBe('Terminal')
    await act(async () => {
      fireEvent.click(button)
    })
    expect(menu.items.length).toBeGreaterThan(0)
    for (const item of menu.items) expect(item.badge === undefined || item.badge === 'Beta').toBe(true)
    expect(menu.items.find((i) => i.id === 'thread')?.badge).toBeUndefined()
    expect(menu.items.find((i) => i.id === 'mute-thread')?.label).toBe('Mute Thread')
  })

  it('a waiting agent: the amber marker is in the main nav, not on the view button or its rows', async () => {
    seed(entry({ unread: 0, waiting: 1 }))
    const nav = render(<WorkspaceThreadUnreadBadge projectId="p1" />)
    const navWaiting = nav.container.querySelector('[data-unread-waiting]')
    expect(navWaiting).not.toBeNull()
    expect(navWaiting?.getAttribute('title')).toBe('1 waiting on you')
    expect(navWaiting?.className).toContain('bg-[var(--color-status-needs-you)]')

    const button = renderMenu()
    expect(button.querySelector('[data-unread-waiting]')).toBeNull()
    expect(button.querySelector('[data-unread-badge]')).toBeNull()
    await act(async () => {
      fireEvent.click(button)
    })
    expect(menu.items.find((i) => i.id === 'thread')?.badge).toBeUndefined()
  })

  it('waiting plus unread: the main nav shows both, the view button and rows neither', async () => {
    seed(entry({ unread: 3, waiting: 2 }))
    const nav = render(<WorkspaceThreadUnreadBadge projectId="p1" />)
    expect(nav.container.querySelector('[data-unread-waiting]')?.textContent).toBe('2')
    expect(nav.container.querySelector('[data-unread-count]')?.textContent).toBe('3')

    const button = renderMenu()
    expect(button.querySelector('[data-unread-badge]')).toBeNull()
    await act(async () => {
      fireEvent.click(button)
    })
    expect(menu.items.find((i) => i.id === 'thread')?.badge).toBeUndefined()
  })

  it('Mute Thread posts /cli/thread/mute and the item flips to Unmute Thread', async () => {
    seed(entry({ unread: 2 }))
    const button = renderMenu()
    menu.pick = 'mute-thread'
    await act(async () => {
      fireEvent.click(button)
    })
    await waitFor(() => expect(cli.post).toHaveBeenCalledWith('thread/mute', { kind: 'thread', id: 'c1', muted: true }))
    await waitFor(() => expect(useUnreadStore.getState().byKey['thread:c1']?.muted).toBe(true))
    expect(button.querySelector('[data-unread-badge]')).toBeNull()
    menu.pick = null
    await act(async () => {
      fireEvent.click(button)
    })
    expect(menu.items.find((i) => i.id === 'mute-thread')?.label).toBe('Unmute Thread')
  })

  it('no Thread id: no count, no mute item', async () => {
    seed(entry({}))
    const { getByTestId } = render(
      <SessionViewMenu
        value="terminal"
        splitLeft="terminal"
        splitRight="thread"
        chatEligible={false}
        onChange={() => {}}
        onSplitLeft={() => {}}
        onSplitRight={() => {}}
      />,
    )
    const button = getByTestId('session-view-button')
    expect(button.querySelector('[data-unread-badge]')).toBeNull()
    await act(async () => {
      fireEvent.click(button)
    })
    expect(menu.items.some((i) => i.id === 'mute-thread')).toBe(false)
  })
})

describe('useAutoMarkRead', () => {
  function reads(): unknown[][] {
    return cli.post.mock.calls.filter(([path]) => path === 'thread/read' || path === 'project-group/read')
  }

  it('visible + focused with a newer seq posts read once with that seq', async () => {
    seed(entry({ lastReadSeq: 3, unread: 2 }))
    const { rerender } = renderHook((p: { seq: number }) =>
      useAutoMarkRead({ kind: 'thread', id: 'c1', newestSeq: p.seq, visible: true }),
    { initialProps: { seq: 5 } })
    await flush()
    expect(reads()).toEqual([['thread/read', { conversationId: 'c1', seq: 5 }]])
    // The same seq again (a re-render) posts nothing.
    rerender({ seq: 5 })
    await flush()
    expect(reads()).toHaveLength(1)
    // A newer message after the read arrives with a frame (unread again).
    act(() => {
      useUnreadStore.getState().applyState(entry({ lastReadSeq: 5, lastSeq: 6, unread: 1 }), { total: 1, waitingTotal: 0 })
    })
    rerender({ seq: 6 })
    await flush()
    expect(reads()).toEqual([
      ['thread/read', { conversationId: 'c1', seq: 5 }],
      ['thread/read', { conversationId: 'c1', seq: 6 }],
    ])
  })

  it('not focused posts nothing; focusing the window posts', async () => {
    seed(entry({ lastReadSeq: 3, unread: 2 }))
    focused = false
    renderHook(() => useAutoMarkRead({ kind: 'thread', id: 'c1', newestSeq: 5, visible: true }))
    await flush()
    expect(reads()).toEqual([])
    focused = true
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await flush()
    expect(reads()).toEqual([['thread/read', { conversationId: 'c1', seq: 5 }]])
  })

  it('not visible (pane hidden / chat panel collapsed) posts nothing', async () => {
    seed(entry({ kind: 'project', id: 'g1', projectId: null, lastReadSeq: 1, unread: 4 }))
    const { rerender } = renderHook((p: { visible: boolean }) =>
      useAutoMarkRead({ kind: 'project', id: 'g1', newestSeq: 5, visible: p.visible }),
    { initialProps: { visible: false } })
    await flush()
    expect(reads()).toEqual([])
    rerender({ visible: true })
    await flush()
    expect(reads()).toEqual([['project-group/read', { group: 'g1', seq: 5 }]])
  })

  it('a seq not past the cursor, or a conversation with nothing unread, posts nothing', async () => {
    seed(entry({ lastReadSeq: 5, unread: 0, waiting: 1 }))
    renderHook(() => useAutoMarkRead({ kind: 'thread', id: 'c1', newestSeq: 5, visible: true }))
    renderHook(() => useAutoMarkRead({ kind: 'thread', id: 'absent', newestSeq: 9, visible: true }))
    await flush()
    expect(reads()).toEqual([])
  })
})
