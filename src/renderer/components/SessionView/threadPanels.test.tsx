// @vitest-environment jsdom
// Thread view's Chatter and Heartbeats panels: the buttons, one drawer at a
// time, the panels sliding out of the terminal's place, and the lists'
// order and states.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'

const daemon = vi.hoisted(() => ({
  get: vi.fn<(scope: unknown, route: string, params?: Record<string, unknown>) => Promise<unknown>>(),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (scope: unknown, route: string, params?: Record<string, unknown>) => daemon.get(scope, route, params),
}))
vi.mock('@/kessel/daemon-ws', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/kessel/daemon-ws')>()),
  // Chatter's live socket never opens here.
  getDaemonWs: () => new Promise(() => {}),
}))

import { SplitSessionColumns } from './SplitSessionColumns'
import { ThreadDrawerPanel } from './ThreadDrawerPanel'
import { SessionViewChromeContext, type SessionViewChromeValue } from './sessionViewChrome'
import { useSessionViewTab } from './useSessionViewTab'
import { useTerminalRevealPresence } from './threadReveal'
import {
  chatterInOrder,
  chatterLine,
  firedAtSeconds,
  heartbeatInputRows,
  threadDrawerOpen,
  type HeartbeatFireRow,
  type ThreadDrawerKind,
  type ThreadPanelKind,
} from './threadPanels'
import type { OverlayThreadItem } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  daemon.get.mockReset()
  delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate
})

// ── Pure helpers ────────────────────────────────────────────────────────

const chatter = (seq: number, over: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem => ({
  collection: 'chatter',
  seq,
  id: `c${seq}`,
  doc: { id: `c${seq}`, kind: 'chatter', from: 'sales', to: 'ops', body: `body ${seq}`, created_at: 1_000 + seq, ...over },
})

describe('threadDrawerOpen', () => {
  it('a panel wins over the remembered terminal; none open is null', () => {
    expect(threadDrawerOpen(false, null)).toBeNull()
    expect(threadDrawerOpen(true, null)).toBe('terminal')
    expect(threadDrawerOpen(true, 'chatter')).toBe('chatter')
    expect(threadDrawerOpen(false, 'heartbeats')).toBe('heartbeats')
  })
})

describe('chatter rows', () => {
  it("one line: the sender's summary, else the body's first line, cut long", () => {
    expect(chatterLine(chatter(1, { summary: ' Shipped it ', body: 'long\nbody' }).doc)).toBe('Shipped it')
    expect(chatterLine(chatter(1, { body: '\n\n  first line  \nsecond' }).doc)).toBe('first line')
    expect(chatterLine(chatter(1, { body: '' }).doc)).toBe('(no text)')
    const long = chatterLine(chatter(1, { body: 'x'.repeat(400) }).doc)
    expect(long.length).toBe(140)
    expect(long.endsWith('…')).toBe(true)
  })

  it('oldest first, so the newest is at the bottom', () => {
    const order = chatterInOrder([chatter(3), chatter(1), chatter(2)]).map((i) => i.seq)
    expect(order).toEqual([1, 2, 3])
  })
})

describe('heartbeat rows', () => {
  const fire = (id: number, firedAt: string, over: Partial<HeartbeatFireRow> = {}): HeartbeatFireRow => ({
    id,
    agentName: 'k2',
    scheduleName: 'daily-brief',
    firedAt,
    decision: 'fired',
    reason: null,
    ...over,
  })

  it('reads RFC 3339 and SQLite times', () => {
    expect(firedAtSeconds('2026-11-09T10:00:00+00:00')).toBe(Date.UTC(2026, 10, 9, 10) / 1000)
    expect(firedAtSeconds('2026-11-09 10:00:00')).toBe(Date.UTC(2026, 10, 9, 10) / 1000)
    expect(firedAtSeconds('nope')).toBeNull()
  })

  it('oldest first (newest at the bottom), edits left out, with source, status and reason', () => {
    const rows = heartbeatInputRows([
      fire(1, '2026-11-09T08:00:00Z'),
      fire(2, '2026-11-09T10:00:00Z', { decision: 'skipped_busy', reason: 'agent busy' }),
      fire(3, '2026-11-09T11:00:00Z', { decision: 'changed' }),
      fire(4, '2026-11-09T09:00:00Z', { decision: 'error', agentName: null, scheduleName: null }),
    ])
    expect(rows.map((r) => r.id)).toEqual(['heartbeat:1', 'heartbeat:4', 'heartbeat:2'])
    expect(rows[2]).toMatchObject({ kind: 'heartbeat', source: 'k2 / daily-brief', status: 'Skipped', tone: 'muted', detail: 'agent busy' })
    expect(rows[1]).toMatchObject({ source: 'Workspace', status: 'Failed', tone: 'error', detail: null })
    expect(rows[0]).toMatchObject({ status: 'Ran', tone: 'ok' })
  })
})

// ── One drawer at a time ────────────────────────────────────────────────

describe('useSessionViewTab: one drawer at a time', () => {
  const PRIMARY = { isPrimary: true, hostKey: 'local' }
  beforeEach(() => localStorage.clear())

  it('opening a panel closes the other panel and the terminal', () => {
    const h = renderHook(() => useSessionViewTab('conv-a', PRIMARY))
    expect(h.result.current.threadPanel).toBeNull()
    act(() => h.result.current.setTerminalRevealed(true))
    act(() => h.result.current.setThreadPanel('chatter'))
    expect(h.result.current.threadPanel).toBe('chatter')
    expect(h.result.current.terminalRevealed).toBe(false)
    expect(localStorage.getItem('local|k2:session-view-terminal:conv-a')).not.toBe('1')
    act(() => h.result.current.setThreadPanel('heartbeats'))
    expect(h.result.current.threadPanel).toBe('heartbeats')
  })

  it('revealing the terminal closes the open panel', () => {
    const h = renderHook(() => useSessionViewTab('conv-a', PRIMARY))
    act(() => h.result.current.setThreadPanel('heartbeats'))
    act(() => h.result.current.setTerminalRevealed(true))
    expect(h.result.current.threadPanel).toBeNull()
    expect(h.result.current.terminalRevealed).toBe(true)
  })

  it('another conversation opens with no panel', () => {
    const h = renderHook(({ key }: { key: string }) => useSessionViewTab(key, PRIMARY), { initialProps: { key: 'conv-a' } })
    act(() => h.result.current.setThreadPanel('chatter'))
    h.rerender({ key: 'conv-b' })
    expect(h.result.current.threadPanel).toBeNull()
  })
})

// The buttons are ThreadDrawerSelector's (ThreadDrawerSelector.test.tsx).

// ── The panel column ────────────────────────────────────────────────────

let animations: Array<{ el: Element; frames: Keyframe[]; reversed: number; onfinish: (() => void) | null; finish: () => void }> = []

let setDrawer: (next: ThreadDrawerKind | null) => void = () => {}

function Frame(): React.JSX.Element {
  const [open, setOpen] = useState<ThreadDrawerKind | null>(null)
  const [last, setLast] = useState<ThreadDrawerKind>('terminal')
  if (open && open !== last) setLast(open)
  setDrawer = setOpen
  const presence = useTerminalRevealPresence(open !== null, true)
  const kind = open ?? last
  return (
    <div data-thread-floats-host="">
      <div data-thread-floats="" />
      <SplitSessionColumns
        showSplit={false}
        hidePty
        left="terminal"
        right="thread"
        terminal={<textarea data-testid="pty" />}
        sharedTerminal={<div />}
        renderView={() => <div />}
        single={<div data-testid="thread" />}
        reveal={{ kind, open: open !== null, shown: presence.shown, motionKey: 'conv', onClosed: presence.onClosed }}
        panel={kind !== 'terminal' ? <div data-testid={`panel-${kind}`} /> : null}
      />
    </div>
  )
}

describe('panels slide out of the terminal’s place', () => {
  beforeEach(() => {
    animations = []
    vi.stubGlobal('matchMedia', (q: string) => ({ matches: !q.includes('reduce') && false, addEventListener() {}, removeEventListener() {} }))
    window.matchMedia = globalThis.matchMedia
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} unobserve() {} })
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 0, 1200, 800))
    HTMLElement.prototype.animate = function animate(this: HTMLElement, frames: Keyframe[]) {
      const a = {
        el: this,
        frames,
        reversed: 0,
        onfinish: null as (() => void) | null,
        cancel() {},
        reverse() {
          a.reversed += 1
        },
        finish() {
          a.onfinish?.()
        },
      }
      animations.push(a)
      return a as unknown as Animation
    } as typeof HTMLElement.prototype.animate
  })

  const column = (): HTMLElement | null => screen.queryByTestId('session-reveal-panel')

  it('opens on the left at the terminal’s size, slides the panel and keeps the floats off it', () => {
    render(<Frame />)
    act(() => setDrawer('chatter'))
    expect(screen.getByTestId('session-reveal-terminal').style.display).toBe('none')
    expect(column()?.style.width).toBe('540px')
    expect(column()?.getAttribute('data-thread-drawer')).toBe('chatter')
    expect(screen.getByTestId('panel-chatter')).toBeTruthy()
    expect(animations[0].el).toBe(column())
    expect(animations[0].frames[0].transform).toBe('translateX(-100%)')
    const host = document.querySelector<HTMLElement>('[data-thread-floats-host]')!
    expect(host.style.getPropertyValue('--thread-float-left')).toBe('540px')
    expect(animations.at(-1)?.frames).toEqual([{ left: '0px' }, { left: '540px' }])
  })

  it('swapping between open drawers doesn’t slide, and only one shows', () => {
    render(<Frame />)
    act(() => setDrawer('terminal'))
    act(() => animations[0].finish())
    animations = []
    act(() => setDrawer('heartbeats'))
    expect(animations).toHaveLength(0)
    expect(screen.getByTestId('session-reveal-terminal').style.display).toBe('none')
    expect(screen.getByTestId('panel-heartbeats')).toBeTruthy()
    act(() => setDrawer('terminal'))
    expect(column()).toBeNull()
    expect(screen.getByTestId('session-reveal-terminal').style.display).toBe('flex')
  })

  it('closing slides the panel shut, then removes it', () => {
    render(<Frame />)
    act(() => setDrawer('heartbeats'))
    act(() => animations[0].finish())
    animations = []
    act(() => setDrawer(null))
    expect(screen.getByTestId('panel-heartbeats')).toBeTruthy()
    expect(animations[0].el).toBe(column())
    expect(animations[0].reversed).toBe(1)
    act(() => animations[0].finish())
    expect(column()).toBeNull()
  })
})

// ── The panels' lists ───────────────────────────────────────────────────

const project = { id: 'p1', path: '/w/sales', workspaces: [{ worktreePath: '/w/sales-wt' }] } as unknown as ProjectWithWorkspaces
const room = testRoom({ tabs: {}, scope: primaryScope(), projects: [project] })

function renderPanel(kind: ThreadPanelKind, chrome: Partial<SessionViewChromeValue> = {}): { close: ReturnType<typeof vi.fn> } {
  const close = vi.fn()
  const value: SessionViewChromeValue = {
    viewTab: 'thread',
    splitLeft: 'terminal',
    splitRight: 'thread',
    overlayAddr: 'sales',
    conversationId: 'conv',
    agentName: 'sales',
    threadPanel: kind,
    setThreadPanel: close,
    projectPath: '/w/sales',
    ...chrome,
  }
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={value}>
      <ThreadDrawerPanel kind={kind} active />
    </SessionViewChromeContext.Provider>,
  )
  return { close }
}

describe('Chatter panel', () => {
  it('loading, then newest at the bottom with sender → recipient and one line', async () => {
    let answer: (v: unknown) => void = () => {}
    daemon.get.mockImplementation(() => new Promise((r) => (answer = r)))
    renderPanel('chatter')
    expect(screen.getByTestId('thread-drawer-loading').textContent).toMatch(/Loading Chatter/)
    expect(daemon.get).toHaveBeenCalledWith(expect.anything(), 'chatter', expect.objectContaining({ addr: 'sales' }))
    await act(async () =>
      answer({
        conversation_id: '',
        has_more: false,
        items: [chatter(2, { summary: 'Second' }), chatter(1, { from: 'ops', to: 'sales', body: 'First line\nmore' })],
      }),
    )
    const rows = screen.getAllByTestId('thread-drawer-row')
    expect(rows.map((r) => r.getAttribute('data-seq'))).toEqual(['1', '2'])
    expect(rows[0].textContent).toContain('ops → sales')
    expect(rows[1].textContent).toContain('Second')
  })

  it('a row opens to the whole message and closes again', async () => {
    daemon.get.mockResolvedValue({ items: [chatter(1, { body: 'Short line\nand the rest of it' })] })
    renderPanel('chatter')
    const row = await screen.findByRole('button', { expanded: false })
    expect(row.textContent).toContain('Short line')
    expect(row.textContent).not.toContain('and the rest')
    fireEvent.click(row)
    expect(row.getAttribute('aria-expanded')).toBe('true')
    expect(row.textContent).toContain('and the rest of it')
    fireEvent.click(row)
    expect(row.getAttribute('aria-expanded')).toBe('false')
  })

  it('empty says what will appear', async () => {
    daemon.get.mockResolvedValue({ items: [] })
    renderPanel('chatter')
    expect((await screen.findByTestId('thread-drawer-empty')).textContent).toMatch(/No messages between agents yet/)
  })

  it('an error offers a retry that reads again', async () => {
    daemon.get.mockRejectedValueOnce(new Error('daemon down'))
    renderPanel('chatter')
    expect((await screen.findByTestId('thread-drawer-error')).textContent).toContain('daemon down')
    daemon.get.mockResolvedValue({ items: [chatter(1)] })
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByTestId('thread-drawer-row')).toBeTruthy()
  })

  it('Escape and the close button close it', async () => {
    daemon.get.mockResolvedValue({ items: [] })
    const { close } = renderPanel('chatter')
    await screen.findByTestId('thread-drawer-empty')
    fireEvent.keyDown(screen.getByTestId('thread-drawer-panel'), { key: 'Escape' })
    expect(close).toHaveBeenCalledWith(null)
    close.mockClear()
    fireEvent.click(screen.getByRole('button', { name: 'Close Chatter' }))
    expect(close).toHaveBeenCalledWith(null)
  })
})

describe('Heartbeats panel', () => {
  it('reads the workspace’s fires (a worktree cwd by its workspace), newest at the bottom', async () => {
    daemon.get.mockResolvedValue([
      { id: 1, agentName: 'sales', scheduleName: 'brief', firedAt: '2026-11-09T08:00:00Z', decision: 'fired' },
      { id: 2, agentName: 'sales', scheduleName: 'eod', firedAt: '2026-11-09T09:00:00Z', decision: 'error', reason: 'boom' },
    ])
    renderPanel('heartbeats', { projectPath: '/w/sales-wt' })
    const rows = await screen.findAllByTestId('thread-drawer-row')
    expect(daemon.get).toHaveBeenCalledWith(expect.anything(), 'heartbeat/fires-list', { project: '/w/sales', limit: 50 })
    expect(rows.map((r) => r.textContent)).toEqual([
      expect.stringContaining('sales / brief'),
      expect.stringContaining('sales / eod'),
    ])
    expect(rows[1].textContent).toContain('Failed · boom')
    expect(rows[1].getAttribute('data-kind')).toBe('heartbeat')
    expect(screen.getByTestId('thread-drawer-coming').textContent).toMatch(/Reminders and mail from your VIP senders/)
  })

  it('loading, then the empty state with what will appear', async () => {
    let answer: (v: unknown) => void = () => {}
    daemon.get.mockImplementation(() => new Promise((r) => (answer = r)))
    renderPanel('heartbeats')
    expect(screen.getByTestId('thread-drawer-loading').textContent).toMatch(/Loading heartbeats/)
    await act(async () => answer([]))
    expect(screen.getByTestId('thread-drawer-empty').textContent).toMatch(/No heartbeats have run yet/)
    expect(screen.getByTestId('thread-drawer-coming')).toBeTruthy()
  })

  it('an error offers a retry', async () => {
    daemon.get.mockRejectedValueOnce(new Error('nope'))
    renderPanel('heartbeats')
    expect((await screen.findByTestId('thread-drawer-error')).textContent).toContain('nope')
    daemon.get.mockResolvedValue([{ id: 1, firedAt: '2026-11-09T08:00:00Z', decision: 'fired' }])
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getAllByTestId('thread-drawer-row')).toHaveLength(1))
  })

  it('re-reads while open (no stream for fires)', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      daemon.get.mockResolvedValue([])
      renderPanel('heartbeats')
      await act(async () => {})
      const before = daemon.get.mock.calls.length
      await act(async () => {
        vi.advanceTimersByTime(15_000)
      })
      expect(daemon.get.mock.calls.length).toBe(before + 1)
    } finally {
      vi.useRealTimers()
    }
  })
})
