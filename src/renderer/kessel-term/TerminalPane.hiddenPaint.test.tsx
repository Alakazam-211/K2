// @vitest-environment jsdom
// P4 (prd-projects-threads-v1): a hidden grid stops drawing and keeps its
// session (`hiddenPaint.ts`). The real TerminalPane under jsdom; only its
// I/O boundaries are mocked (same set as TerminalPane.threadView.test.tsx).
// The pause delay is shortened here; its real value is pinned below.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, waitFor } from '@testing-library/react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import {
  SessionViewChromeContext,
  type SessionViewChromeValue,
} from '@/components/SessionView/sessionViewChrome'

const DELAY = 150
vi.mock('./hiddenPaint', async (importOriginal) => {
  const real = await importOriginal<typeof import('./hiddenPaint')>()
  return { ...real, HIDDEN_PAINT_PAUSE_MS: 150 }
})

const h = vi.hoisted(() => ({
  painter: 'dom',
  threadMounts: 0,
  sockets: [] as Array<{
    sent: string[]
    readyState: number
    closed: number
    onmessage: ((e: { data: string }) => void) | null
  }>,
  painters: { mounts: 0, disposes: 0 },
}))

vi.mock('@/lib/grid-dial-queue', () => ({
  gridDialBackoffRemainingMs: () => 0,
  openQueuedGridWebSocket: vi.fn(async () => {
    const socket = {
      readyState: 1,
      binaryType: 'blob',
      sent: [] as string[],
      onmessage: null as ((e: { data: string }) => void) | null,
      onopen: null,
      onerror: null,
      onclose: null,
      send(data: string) {
        this.sent.push(data)
      },
      closed: 0,
      close() {
        this.closed += 1
        this.readyState = 3
      },
    }
    h.sockets.push(socket)
    return socket
  }),
}))

vi.mock('./webgl/webglPainter', () => ({
  createWebglPainter: () => ({
    mount: () => {
      h.painters.mounts += 1
    },
    setMetrics: () => {},
    render: () => {},
    onFatal: () => {},
    dispose: () => {
      h.painters.disposes += 1
    },
  }),
}))

vi.mock('@/components/SessionView/ThreadOverlayColumn', async () => {
  const React = await import('react')
  return {
    // The Thread's message box: focus must stay here while the hidden
    // terminal loads.
    ThreadOverlayColumn: () => {
      React.useEffect(() => {
        h.threadMounts += 1
      }, [])
      return React.createElement('textarea', { 'data-testid': 'stub-thread-box' })
    },
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
// Dynamically imported by the drag-drop effect — must be inert or the
// real module's transformCallback rejects outside any test body.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => undefined),
  daemonCliPost: vi.fn(async () => undefined),
}))
// Same module TerminalPane imports as '../kessel/daemon-ws' — this test
// file lives in the same directory, so the specifier resolves identically.
vi.mock('../kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9, token: 'tok', host: '127.0.0.1' })),
  invalidateDaemonWs: vi.fn(),
  daemonHttpBase: () => 'http://127.0.0.1:9',
  daemonWsBase: () => 'ws://127.0.0.1:9',
}))
vi.mock('@/lib/remote-session', () => ({
  isPossibleAuthFailure: () => false,
  isPasswordChangeRequired: () => false,
  requirePasswordRotation: vi.fn(),
  reviveRemoteSession: vi.fn(async () => 'still-valid'),
}))
vi.mock('@/lib/file-drag', () => ({
  bracketPaste: (t: string) => t,
  isImagePath: () => false,
  quotePathForImageDrop: (p: string) => p,
}))
vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: vi.fn(async () => undefined),
}))
vi.mock('@/components/Terminal/TerminalComposeBar', () => ({
  TerminalComposeBar: () => null,
}))

// Stores — selector-hook and/or getState() shapes, matching how
// TerminalPane consumes each one.
vi.mock('@/stores/terminal-settings', () => {
  const state = {
    fontSize: 13,
    linkClickMode: 'click',
    get painter() { return h.painter },
    openLinksInSplitPane: false,
  }
  return {
    useTerminalSettingsStore: Object.assign(
      (sel: (s: typeof state) => unknown) => sel(state),
      { getState: () => state },
    ),
  }
})
const tabsApi = vi.hoisted(() => ({
  releasePaneOwnedElsewhere: vi.fn(),
}))
vi.mock('@/stores/tabs', () => ({
  useTabsStore: {
    getState: () => ({
      setTerminalSandboxBackend: vi.fn(),
      setTerminalConversationId: vi.fn(),
      setTabTitle: vi.fn(),
      tabs: [],
      extraGroups: [],
      releasePaneOwnedElsewhere: tabsApi.releasePaneOwnedElsewhere,
    }),
  },
}))
vi.mock('@/stores/window-focus', () => ({
  useWindowFocusStore: {
    getState: () => ({ isFocused: true }),
    subscribe: () => () => undefined,
  },
}))
vi.mock('@/stores/session-labels', () => ({
  useSessionLabelsStore: {
    getState: () => ({ setSessionLabel: vi.fn() }),
  },
}))
vi.mock('@/stores/active-agents', () => ({
  useActiveAgentsStore: {
    getState: () => ({
      agents: new Map(),
    }),
  },
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: {
    getState: () => ({ activeHost: 'local' }),
  },
  // S5 — the window-mode store (imported via TerminalPane) registers a
  // host-switch listener at module scope.
  onActiveHostChange: () => () => {},
}))


import { TerminalPane } from './TerminalPane'
import { shouldPauseHiddenPaint } from './hiddenPaint'
import { renderInRoom, fixedStore, testRoom } from '@/test-utils/room'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

const room = testRoom({
  tabs: useTabsStore,
  activity: { setViewing: vi.fn() },
  presence: fixedStore({ roster: [], supported: true }),
})

class StubResizeObserver {
  constructor(public cb: ResizeObserverCallback) {}
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
class StubWebSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  readyState = 0
  send(): void {}
  close(): void {}
}

const CELL_W = 8
const CELL_H = 16
const realRect = HTMLElement.prototype.getBoundingClientRect
beforeEach(() => {
  vi.stubGlobal('ResizeObserver', StubResizeObserver)
  vi.stubGlobal('WebSocket', StubWebSocket)
  globalThis.fetch = vi.fn(async () => ({
    ok: true,
    status: 200,
    json: async () => ({ sessionId: 's1', agentName: 'tab-test', cols: 0, rows: 0, reused: false }),
    text: async () => '',
  })) as unknown as typeof fetch
  h.painter = 'dom'
  h.threadMounts = 0
  h.sockets = []
  h.painters = { mounts: 0, disposes: 0 }
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    if (this.tagName === 'SPAN' && this.textContent === 'W' && this.style?.visibility === 'hidden') {
      return new DOMRect(0, 0, CELL_W, CELL_H)
    }
    return new DOMRect(0, 0, 0, 0)
  }
  useSettingsStore.setState({ settingsOpen: false })
})
afterEach(() => {
  HTMLElement.prototype.getBoundingClientRect = realRect
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

function chrome(over: Partial<SessionViewChromeValue>): SessionViewChromeValue {
  return {
    viewTab: 'thread',
    splitLeft: 'terminal',
    splitRight: 'thread',
    overlayAddr: 'sales',
    conversationId: 'c1',
    agentName: 'sales',
    displayName: 'Sales',
    terminalRevealed: false,
    terminalRevealKey: 'key-1',
    ...over,
  }
}
function pane(value: SessionViewChromeValue, tile = false): React.JSX.Element {
  return (
    <TabVisibilityContext.Provider value={true}>
      <SessionViewChromeContext.Provider value={value}>
        <TerminalPane terminalId="pg-test" cwd="/tmp/ws" pauseHiddenPaint={tile || undefined} />
      </SessionViewChromeContext.Provider>
    </TabVisibilityContext.Provider>
  )
}

const run = (text: string) => ({ text, fg: null, bg: null, bold: false, italic: false, underline: false, inverse: false, dim: false, strikeout: false })
const grid = (lines: string[]) => Array.from({ length: 24 }, (_, i) => [run(lines[i] ?? '')])
let version = 1
function send(frame: unknown): void {
  act(() => {
    h.sockets[0].onmessage?.({ data: JSON.stringify(frame) })
  })
}
function snapshotFrame(lines: string[]): unknown {
  version += 1
  return {
    event: 'snapshot',
    payload: {
      paneId: 'p', cols: 80, rows: 24, grid: grid(lines), scrollback: [],
      cursor: { row: 0, col: 0, visible: true }, version, displayOffset: 0,
      mouseReport: false, sgrMouse: false, altScreen: false,
    },
  }
}
function deltaFrame(row: number, text: string): unknown {
  version += 1
  return {
    event: 'delta',
    payload: {
      paneId: 'p', cols: 80, rows: 24, damagedRows: [{ row, runs: [run(text)] }],
      scrollbackAppended: [], cursor: { row: 0, col: 0, visible: true }, version, displayOffset: 0,
    },
  }
}
/** Let the rAF frame coalescer flush. */
async function frame(): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 40))
  })
}
async function wait(ms: number): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, ms))
  })
}
async function connected(): Promise<void> {
  await waitFor(() => expect(h.sockets.length).toBe(1))
  await waitFor(() => expect(h.sockets[0].onmessage).toBeTruthy())
}
/** The grid text the DOM painter has committed. */
const drawn = (): string => document.querySelector('[data-terminal-container]')?.textContent ?? ''
const spawnCalls = (): number => vi.mocked(globalThis.fetch).mock.calls.length

describe('hiddenPaint rules', () => {
  it('pauses only a hidden grid, and only when the caller opts in', () => {
    expect(shouldPauseHiddenPaint({ enabled: true, terminalOnScreen: false })).toBe(true)
    expect(shouldPauseHiddenPaint({ enabled: true, terminalOnScreen: true })).toBe(false)
    expect(shouldPauseHiddenPaint({ enabled: false, terminalOnScreen: false })).toBe(false)
  })
  it('waits 2s', async () => {
    const real = await vi.importActual<typeof import('./hiddenPaint')>('./hiddenPaint')
    expect(real.HIDDEN_PAINT_PAUSE_MS).toBe(2000)
  })
})

describe('A Projects tile’s hidden terminal', () => {
  it('pauses after the delay, not before', async () => {
    const { rerender } = renderInRoom(room, pane(chrome({ viewTab: 'terminal' }), true))
    await connected()
    send(snapshotFrame(['tick 1']))
    await frame()
    expect(drawn()).toContain('tick 1')

    rerender(pane(chrome({ viewTab: 'thread' }), true))
    send(deltaFrame(1, 'tick 2'))
    await frame()
    expect(drawn()).toContain('tick 2') // within the delay: still drawing

    await wait(DELAY + 20)
    send(deltaFrame(2, 'tick 3'))
    await frame()
    expect(drawn()).not.toContain('tick 3') // paused
  })

  it('never pauses while the terminal is showing', async () => {
    renderInRoom(room, pane(chrome({ viewTab: 'terminal' }), true))
    await connected()
    send(snapshotFrame(['tick 1']))
    await wait(DELAY * 2)
    send(deltaFrame(1, 'tick 2'))
    await frame()
    expect(drawn()).toContain('tick 2')
  })

  it('a quick peek away and back never pauses', async () => {
    const { rerender } = renderInRoom(room, pane(chrome({ viewTab: 'terminal' }), true))
    await connected()
    send(snapshotFrame(['tick 1']))
    await frame()
    rerender(pane(chrome({ viewTab: 'thread' }), true))
    await wait(DELAY / 3)
    rerender(pane(chrome({ viewTab: 'terminal' }), true))
    await wait(DELAY + 20)
    send(deltaFrame(1, 'tick 2'))
    await frame()
    expect(drawn()).toContain('tick 2')
  })

  it('keeps the live grid up to date while paused, and draws all of it the moment it shows', async () => {
    const { rerender } = renderInRoom(room, pane(chrome({ viewTab: 'terminal' }), true))
    await connected()
    send(snapshotFrame(['tick 1']))
    await frame()
    rerender(pane(chrome({ viewTab: 'thread' }), true))
    await wait(DELAY + 20)
    send(deltaFrame(1, 'row one while paused'))
    send(deltaFrame(2, 'row two while paused'))
    await frame()
    send(deltaFrame(3, 'last line, not yet flushed'))
    expect(drawn()).not.toContain('while paused')

    // The show commit itself carries the current grid: no frame, no wait.
    rerender(pane(chrome({ viewTab: 'terminal' }), true))
    const text = drawn()
    expect(text).toContain('row one while paused')
    expect(text).toContain('row two while paused')
    expect(text).toContain('last line, not yet flushed')
    expect(text).toContain('tick 1')
  })

  it('hiding closes nothing and tells the daemon nothing', async () => {
    const { rerender } = renderInRoom(room, pane(chrome({ viewTab: 'terminal' }), true))
    await connected()
    send(snapshotFrame(['tick 1']))
    await frame()
    const sentBefore = h.sockets[0].sent.slice()
    const fetchBefore = spawnCalls()
    const getBefore = vi.mocked(daemonCliGet).mock.calls.length
    const postBefore = vi.mocked(daemonCliPost).mock.calls.length

    rerender(pane(chrome({ viewTab: 'thread' }), true))
    await wait(DELAY + 20)
    for (let i = 0; i < 10; i++) send(deltaFrame(i, `tick ${i + 10}`))
    await frame()

    expect(h.sockets).toHaveLength(1)
    expect(h.sockets[0].closed).toBe(0)
    expect(h.sockets[0].readyState).toBe(1)
    expect(h.sockets[0].sent).toEqual(sentBefore)
    expect(spawnCalls()).toBe(fetchBefore)
    expect(vi.mocked(daemonCliGet).mock.calls.length).toBe(getBefore)
    expect(vi.mocked(daemonCliPost).mock.calls.length).toBe(postBefore)

    // Showing again sends nothing either (same size, no resize).
    rerender(pane(chrome({ viewTab: 'terminal' }), true))
    await frame()
    expect(h.sockets[0].closed).toBe(0)
    expect(h.sockets[0].sent).toEqual(sentBefore)
  })
})
