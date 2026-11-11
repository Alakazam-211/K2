// @vitest-environment jsdom
// Thread view inside TerminalPane (prd-thread-view-focus-v1): what the pane
// hands its Thread in each view. The real TerminalPane under jsdom; only its
// I/O boundaries are mocked (same set as TerminalPane.spawn.test.tsx).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, waitFor } from '@testing-library/react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import {
  SessionViewChromeContext,
  type SessionViewChromeValue,
} from '@/components/SessionView/sessionViewChrome'
import { THREAD_TEXT_WIDTH_DEFAULT } from '@/components/SessionView/threadTextWidth'

const h = vi.hoisted(() => ({
  painter: 'dom',
  threadProps: [] as Array<Record<string, unknown>>,
  threadMounts: 0,
  composeProps: [] as Array<Record<string, unknown>>,
  sockets: [] as Array<{ sent: string[]; onmessage: ((e: { data: string }) => void) | null }>,
  painters: { mounts: 0, disposes: 0, fatal: null as ((reason: string) => void) | null },
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
      close() {
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
    onFatal: (cb: (reason: string) => void) => {
      h.painters.fatal = cb
    },
    dispose: () => {
      h.painters.disposes += 1
    },
  }),
}))

vi.mock('@/components/SessionView/ThreadOverlayColumn', async () => {
  const React = await import('react')
  return {
    ThreadOverlayColumn: (props: Record<string, unknown>) => {
      h.threadProps.push(props)
      React.useEffect(() => {
        h.threadMounts += 1
      }, [])
      return React.createElement('div', { 'data-testid': 'stub-thread' }, 'thread')
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
  TerminalComposeBar: (props: Record<string, unknown>) => {
    h.composeProps.push(props)
    return null
  },
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
import { fixedStore, inRoom, renderInRoom, testRoom } from '@/test-utils/room'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { daemonCliGet } from '@/lib/daemon-cli'
import { CONTROL_CHAR_REFUSAL } from '@/lib/drop-payload'

const room = testRoom({
  tabs: useTabsStore,
  activity: { setViewing: vi.fn() },
  presence: fixedStore({ roster: [], supported: true }),
})

class StubResizeObserver {
  static all: StubResizeObserver[] = []
  el: Element | null = null
  constructor(public cb: ResizeObserverCallback) {
    StubResizeObserver.all.push(this)
  }
  observe(el: Element): void {
    this.el = el
  }
  unobserve(): void {}
  disconnect(): void {
    StubResizeObserver.all = StubResizeObserver.all.filter((o) => o !== this)
  }
}

/** Fire the terminal container's observer (TerminalPane's PTY-resize one). */
function fireTerminalRO(width: number, height: number): void {
  const entries = [{ contentRect: { width, height } }] as unknown as ResizeObserverEntry[]
  for (const o of StubResizeObserver.all.slice()) {
    if (o.el?.hasAttribute('data-terminal-container')) o.cb(entries, o as unknown as ResizeObserver)
  }
}

class StubWebSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  binaryType = 'blob'
  readyState = 0
  onopen: (() => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null
  onmessage: (() => void) | null = null
  send(): void {}
  close(): void {
    this.readyState = 3
  }
}

function chrome(over: Partial<SessionViewChromeValue>): SessionViewChromeValue {
  return {
    viewTab: 'thread',
    splitLeft: 'terminal',
    splitRight: 'thread',
    overlayAddr: 'sales',
    conversationId: 'c1',
    agentName: 'sales',
    displayName: 'Sales',
    ...over,
  }
}

function pane(value: SessionViewChromeValue): React.JSX.Element {
  return (
    <TabVisibilityContext.Provider value={true}>
      <SessionViewChromeContext.Provider value={value}>
        <TerminalPane terminalId="pg-test" cwd="/tmp/ws" />
      </SessionViewChromeContext.Provider>
    </TabVisibilityContext.Provider>
  )
}

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
  h.threadProps = []
  h.threadMounts = 0
  h.composeProps = []
  h.sockets = []
  h.painters = { mounts: 0, disposes: 0, fatal: null }
  StubResizeObserver.all = []
  installCellProbe()
  useSettingsStore.setState({ threadTextWidth: THREAD_TEXT_WIDTH_DEFAULT, settingsOpen: false })
})

afterEach(() => {
  restoreCellProbe()
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

// Cell probe: 8×16 cells, so measurePaneFit gives real cols/rows.
const CELL_W = 8
const CELL_H = 16
const realRect = HTMLElement.prototype.getBoundingClientRect
function installCellProbe(): void {
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    if (this.tagName === 'SPAN' && this.textContent === 'W' && this.style?.visibility === 'hidden') {
      return new DOMRect(0, 0, CELL_W, CELL_H)
    }
    return new DOMRect(0, 0, 0, 0)
  }
}
function restoreCellProbe(): void {
  HTMLElement.prototype.getBoundingClientRect = realRect
}

const SPAWN = (): number =>
  vi.mocked(globalThis.fetch).mock.calls.filter(([u]) => String(u).includes('/cli/sessions/v2/spawn')).length

/** Feed a snapshot so the pane reaches `ready`. */
async function goReady(): Promise<void> {
  await waitFor(() => expect(h.sockets.length).toBe(1))
  await waitFor(() => expect(h.sockets[0].onmessage).toBeTruthy())
  act(() => {
    h.sockets[0].onmessage?.({
      data: JSON.stringify({
        event: 'snapshot',
        payload: {
          paneId: 'p',
          cols: 80,
          rows: 24,
          grid: [],
          scrollback: [],
          cursor: { row: 0, col: 0, visible: true },
          version: 1,
          displayOffset: 0,
          mouseReport: false,
          sgrMouse: false,
          altScreen: false,
        },
      }),
    })
  })
  await waitFor(() => expect(document.querySelector('textarea')).toBeTruthy())
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

const sends = (kind: string): Array<Record<string, unknown>> =>
  h.sockets[0].sent.map((x) => JSON.parse(x) as Record<string, unknown>).filter((m) => m.action === kind)

async function waitDebounce(): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 130))
  })
}

describe('Thread text limit in TerminalPane', () => {
  it('Thread view passes the limit', () => {
    renderInRoom(room, pane(chrome({ viewTab: 'thread' })))
    expect(h.threadProps.at(-1)?.textCapPx).toBe(820)
  })

  it('Full width passes no limit', () => {
    useSettingsStore.setState({ threadTextWidth: { limited: false, px: 720 } })
    renderInRoom(room, pane(chrome({ viewTab: 'thread' })))
    expect(h.threadProps.at(-1)?.textCapPx).toBeNull()
  })

  it('a changed width reaches the open Thread', () => {
    renderInRoom(room, pane(chrome({ viewTab: 'thread' })))
    act(() => useSettingsStore.setState({ threadTextWidth: { limited: true, px: 600 } }))
    expect(h.threadProps.at(-1)?.textCapPx).toBe(600)
  })

  it("Split view's Thread has no limit", () => {
    renderInRoom(room, pane(chrome({ viewTab: 'split' })))
    expect(h.threadProps.length).toBeGreaterThan(0)
    expect(h.threadProps.at(-1)?.textCapPx).toBeUndefined()
  })
})

describe('Revealing the terminal from Thread view', () => {
  const thread = (over: Partial<SessionViewChromeValue> = {}): SessionViewChromeValue =>
    chrome({ viewTab: 'thread', terminalRevealed: false, terminalRevealKey: 'key-1', ...over })

  it('no remount, no second spawn, no new socket across toggles', async () => {
    const { rerender } = renderInRoom(room, pane(thread()))
    await goReady()
    const container = document.querySelector('[data-terminal-container]')
    expect(container).toBeTruthy()
    for (const open of [true, false, true, false]) {
      rerender(pane(thread({ terminalRevealed: open })))
      await act(async () => {})
    }
    expect(document.querySelector('[data-terminal-container]')).toBe(container)
    expect(SPAWN()).toBe(1)
    expect(h.sockets).toHaveLength(1)
    expect(h.sockets[0].sent.filter((s) => s.includes('close'))).toHaveLength(0)
    expect(h.threadMounts).toBe(1)
  })

  it('hidden: 0×0 measures send no resize; reveal sends one, then none at the same size', async () => {
    const { rerender } = renderInRoom(room, pane(thread()))
    await goReady()
    const before = sends('resize').length
    fireTerminalRO(0, 0)
    await waitDebounce()
    expect(sends('resize').length).toBe(before)

    rerender(pane(thread({ terminalRevealed: true })))
    fireTerminalRO(800, 480)
    await waitDebounce()
    const after = sends('resize')
    expect(after.length).toBe(before + 1)
    expect(Number(after.at(-1)?.cols)).toBeGreaterThan(0)
    expect(Number(after.at(-1)?.rows)).toBeGreaterThan(0)

    // Hide, reveal again at the same size: the daemon hears nothing.
    rerender(pane(thread({ terminalRevealed: false })))
    fireTerminalRO(0, 0)
    await waitDebounce()
    rerender(pane(thread({ terminalRevealed: true })))
    fireTerminalRO(800, 480)
    await waitDebounce()
    expect(sends('resize').length).toBe(before + 1)
  })

  it('holds no WebGL surface while hidden, and rebuilds it on reveal', async () => {
    h.painter = 'webgl'
    const { rerender } = renderInRoom(room, pane(thread()))
    await goReady()
    expect(h.painters.mounts).toBe(0)

    rerender(pane(thread({ terminalRevealed: true })))
    expect(h.painters.mounts).toBe(1)
    expect(h.painters.disposes).toBe(0)

    rerender(pane(thread({ terminalRevealed: false })))
    await act(async () => {})
    expect(h.painters.disposes).toBe(1)
  })

  it('a panel takes the terminal’s place: the terminal stays hidden, no resize, no WebGL surface', async () => {
    h.painter = 'webgl'
    const { rerender } = renderInRoom(room, pane(thread({ terminalRevealed: true, threadPanel: null, setThreadPanel: vi.fn() })))
    await goReady()
    expect(h.painters.mounts).toBe(1)
    const before = sends('resize').length

    // Opening Chatter closes the terminal (useSessionViewTab), in one commit.
    rerender(pane(thread({ terminalRevealed: false, threadPanel: 'chatter', setThreadPanel: vi.fn() })))
    await act(async () => {})
    const byId = (id: string): HTMLElement => document.querySelector<HTMLElement>(`[data-testid="${id}"]`)!
    expect(byId('session-reveal-terminal').style.display).toBe('none')
    expect(byId('session-reveal-panel').getAttribute('data-thread-drawer')).toBe('chatter')
    expect(byId('thread-drawer-panel').getAttribute('data-thread-panel')).toBe('chatter')
    expect(h.painters.disposes).toBe(1)
    fireTerminalRO(0, 0)
    await waitDebounce()
    expect(sends('resize').length).toBe(before)
    expect(h.threadMounts).toBe(1)
  })

  it('a lost context while revealed gets its retry on the next reveal', async () => {
    h.painter = 'webgl'
    const { rerender } = renderInRoom(room, pane(thread({ terminalRevealed: true })))
    await goReady()
    expect(h.painters.mounts).toBe(1)
    act(() => h.painters.fatal?.('webgl-context-lost-unrestored'))
    rerender(pane(thread({ terminalRevealed: false })))
    await act(async () => {})
    rerender(pane(thread({ terminalRevealed: true })))
    await act(async () => {})
    expect(h.painters.mounts).toBe(2)
  })

  it('Terminal view keeps its WebGL surface', async () => {
    h.painter = 'webgl'
    renderInRoom(room, pane(chrome({ viewTab: 'terminal', terminalRevealed: false, terminalRevealKey: 'key-1' })))
    await goReady()
    expect(h.painters.mounts).toBe(1)
    expect(h.painters.disposes).toBe(0)
  })

  it('A Split with no Terminal side holds no WebGL surface', async () => {
    h.painter = 'webgl'
    renderInRoom(room, pane(chrome({ viewTab: 'split', splitLeft: 'thread', splitRight: 'chatter' })))
    await goReady()
    expect(h.painters.mounts).toBe(0)
  })

  it('the chord never reaches the PTY in Thread view, rebound or not; Terminal view keeps the key', async () => {
    // Bound to a chord the PTY would otherwise type (^K), so the guard is
    // what stops it, not an unmapped key.
    useSettingsStore.setState({ keybindings: { toggleThreadTerminal: 'Ctrl+K' } })
    const { unmount } = renderInRoom(room, pane(thread({ terminalRevealed: true })))
    await goReady()
    const press = (): void => {
      const input = document.querySelector<HTMLTextAreaElement>('textarea')
      if (!input) throw new Error('no shadow input')
      input.dispatchEvent(new KeyboardEvent('keydown', { key: 'K', code: 'KeyK', ctrlKey: true, bubbles: true }))
    }
    press()
    expect(sends('input')).toHaveLength(0)
    unmount()

    h.sockets = []
    useSettingsStore.setState({ keybindings: { toggleThreadTerminal: 'Ctrl+K' } })
    renderInRoom(room, pane(chrome({ viewTab: 'terminal', terminalRevealed: false, terminalRevealKey: 'key-1' })))
    await goReady()
    press()
    expect(sends('input').length).toBeGreaterThan(0)
  })
})

describe('the compose draft belongs to the pane, not the PTY', () => {
  // A refresh respawns the PTY under a new sessionId and remounts the pane;
  // the bars key their draft by the pane's v2 agent name so it survives.
  function composePane(value: SessionViewChromeValue, attachAgentName?: string): React.JSX.Element {
    return (
      <TabVisibilityContext.Provider value={true}>
        <SessionViewChromeContext.Provider value={value}>
          <TerminalPane terminalId="pg-test" cwd="/tmp/ws" attachAgentName={attachAgentName} showComposeBar />
        </SessionViewChromeContext.Provider>
      </TabVisibilityContext.Provider>
    )
  }

  it("the Thread's bar gets the attach agent name as its draft id", async () => {
    renderInRoom(room, composePane(chrome({ viewTab: 'thread' }), 'sales/reviewer'))
    await goReady()
    const bar = h.threadProps.at(-1)?.composeBar as React.ReactElement<Record<string, unknown>> | undefined
    if (!bar) throw new Error('Thread got no compose bar')
    expect(bar.props.sendDestination).toBe('thread')
    expect(bar.props.sessionId).toBe('s1')
    expect(bar.props.draftId).toBe('sales/reviewer')
  })

  it("the terminal's bar on a plain tab uses the tab's v2 key", async () => {
    renderInRoom(room, composePane(chrome({ viewTab: 'terminal' })))
    await goReady()
    const bar = h.composeProps.at(-1)
    if (!bar) throw new Error('terminal got no compose bar')
    expect(bar.sendDestination).toBe('pty')
    expect(bar.draftId).toBe('tab-pg-test')
  })
})

// Same harness, another concern: the text a file drop or a Finder paste
// types into this terminal (lib/drop-payload.ts builds it; tested there).
describe('files dropped or pasted on the terminal', () => {
  async function terminalPane(): Promise<HTMLElement> {
    renderInRoom(room, pane(chrome({ viewTab: 'terminal' })))
    await goReady()
    const el = document.querySelector<HTMLElement>('[data-terminal-container]')
    if (!el) throw new Error('no terminal container')
    return el
  }

  function dropFiles(el: HTMLElement, paths: string[]): void {
    fireEvent.drop(el, { dataTransfer: { files: paths.map((path) => ({ path })) } })
  }

  async function pasteFinderPaths(paths: string[]): Promise<void> {
    vi.mocked(daemonCliGet).mockImplementation(async (_scope: unknown, path: string) =>
      path === 'fs/clipboard-paths' ? paths : undefined,
    )
    const input = document.querySelector<HTMLTextAreaElement>('textarea')
    if (!input) throw new Error('no shadow input')
    const ev = new Event('paste', { bubbles: true, cancelable: true })
    Object.defineProperty(ev, 'clipboardData', { value: { getData: () => 'ignored' } })
    await act(async () => {
      input.dispatchEvent(ev)
      await new Promise((r) => setTimeout(r, 0))
    })
  }

  const toasts = () => useToastStore.getState().toasts.map((t) => [t.message, t.type])

  beforeEach(() => {
    useToastStore.setState({ toasts: [] })
  })

  it('a dropped file is typed backslash-escaped with a trailing space', async () => {
    const el = await terminalPane()
    dropFiles(el, ['/tmp/my notes.txt'])
    expect(sends('input').map((m) => m.text)).toEqual(['/tmp/my\\ notes.txt '])
  })

  it('a dropped name with a line break: a toast, and nothing typed', async () => {
    const el = await terminalPane()
    dropFiles(el, ['/tmp/notes\nrm -rf ~'])
    expect(sends('input')).toEqual([])
    expect(toasts()).toEqual([[CONTROL_CHAR_REFUSAL, 'warning']])
  })

  it('a Windows path is typed PowerShell-quoted (the daemon opens powershell here)', async () => {
    const el = await terminalPane()
    dropFiles(el, ['C:\\Users\\a\\my notes.txt'])
    expect(sends('input').map((m) => m.text)).toEqual(["'C:\\Users\\a\\my notes.txt' "])
  })

  it('Finder paths pasted: the same text as a drop; a line break is refused', async () => {
    await terminalPane()
    await pasteFinderPaths(['/tmp/a b.txt'])
    expect(sends('input').map((m) => m.text)).toEqual(['/tmp/a\\ b.txt '])

    await pasteFinderPaths(['/tmp/x\ny.txt'])
    expect(sends('input').map((m) => m.text)).toEqual(['/tmp/a\\ b.txt '])
    expect(toasts()).toEqual([[CONTROL_CHAR_REFUSAL, 'warning']])
  })
})
