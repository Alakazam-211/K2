// @vitest-environment jsdom
// GPU leak checks for a terminal pane on the WebGL painter (field report:
// WebKit's GPU process at 21 GB after 16 h, swap growing). The real
// TerminalPane and the real WebGL painter run against the counting fakes in
// test-utils/gpu-ledger; only the pane's I/O boundaries are mocked (the
// same set as TerminalPane.hiddenPaint.test.tsx). Each trigger repeats 100×
// and what the GPU process would hold must come back to baseline: a hidden,
// parked or closed pane holds no context, no drawing buffer and no atlas.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, waitFor } from '@testing-library/react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import {
  SessionViewChromeContext,
  type SessionViewChromeValue,
} from '@/components/SessionView/sessionViewChrome'
import { installGpuLedger, type GpuLedger } from '@/test-utils/gpu-ledger'

const h = vi.hoisted(() => ({
  painter: 'webgl',
  fontSize: 13,
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
    get fontSize() { return h.fontSize },
    // The WebGL cell probe reads these (NaN metrics would skip the painter).
    lineHeightMultiplier: 1.2,
    charTracking: 1,
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
import { renderInRoom, fixedStore, testRoom } from '@/test-utils/room'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'

const REPEAT = 100

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
let ledger: GpuLedger
beforeEach(() => {
  ledger = installGpuLedger()
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  vi.stubGlobal('ResizeObserver', StubResizeObserver)
  vi.stubGlobal('WebSocket', StubWebSocket)
  globalThis.fetch = vi.fn(async () => ({
    ok: true,
    status: 200,
    json: async () => ({ sessionId: 's1', agentName: 'tab-test', cols: 0, rows: 0, reused: false }),
    text: async () => '',
  })) as unknown as typeof fetch
  h.painter = 'webgl'
  h.fontSize = 13
  h.sockets = []
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
  ledger.restore()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  document.body.innerHTML = ''
})

const chrome: SessionViewChromeValue = {
  viewTab: 'terminal',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: 'c1',
  agentName: 'sales',
  displayName: 'Sales',
  terminalRevealed: false,
  terminalRevealKey: 'key-1',
}
function pane(visible: boolean, id = 'pg-test'): React.JSX.Element {
  return (
    <TabVisibilityContext.Provider value={visible}>
      <SessionViewChromeContext.Provider value={chrome}>
        <TerminalPane terminalId={id} cwd="/tmp/ws" />
      </SessionViewChromeContext.Provider>
    </TabVisibilityContext.Provider>
  )
}

const run = (text: string) => ({ text, fg: null, bg: null, bold: false, italic: false, underline: false, inverse: false, dim: false, strikeout: false })
let version = 1
function snapshotFrame(tag: string): unknown {
  version += 1
  return {
    event: 'snapshot',
    payload: {
      paneId: 'p', cols: 80, rows: 24,
      grid: Array.from({ length: 24 }, (_, i) => [run(`${tag} ${i} ┌──┐ ✓`)]),
      scrollback: [],
      cursor: { row: 0, col: 0, visible: true }, version, displayOffset: 0,
      mouseReport: false, sgrMouse: false, altScreen: false,
    },
  }
}
function send(socket: number, frame: unknown): void {
  act(() => {
    h.sockets[socket].onmessage?.({ data: JSON.stringify(frame) })
  })
}
async function frame(): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 40))
  })
}
async function connected(n = 1): Promise<void> {
  await waitFor(() => expect(h.sockets.length).toBe(n))
  await waitFor(() => expect(h.sockets[n - 1].onmessage).toBeTruthy())
}
const atlasPages = (): number => document.querySelectorAll('[data-k2-atlas-page]').length
const glCanvas = (): HTMLCanvasElement | null => document.querySelector('[data-terminal-webgl-canvas]')

/** One visible pane, painted: the baseline every test measures against. */
async function paintedPane() {
  const r = renderInRoom(room, pane(true))
  await connected()
  send(0, snapshotFrame('first'))
  await frame()
  expect(glCanvas(), 'the pane is on the WebGL painter').toBeTruthy()
  expect(ledger.liveGlContexts(), ledger.summary()).toBe(1)
  expect(ledger.textureBytes(), 'the atlas was uploaded').toBeGreaterThan(0)
  return r
}

describe('a WebGL terminal pane gives its GPU memory back', () => {
  it(`hiding and showing the tab ${REPEAT}×: hidden holds nothing, shown holds one context`, async () => {
    const { rerender } = await paintedPane()
    const shown = { objects: ledger.liveGlObjects(), texture: ledger.textureBytes() }
    for (let i = 0; i < REPEAT; i++) {
      rerender(pane(false))
      expect(ledger.liveGlContexts(), `hidden #${i}: ${ledger.summary()}`).toBe(0)
      expect(ledger.drawingBufferBytes()).toBe(0)
      expect(ledger.canvas2dBytes(), `hidden #${i}: ${ledger.summary()}`).toBe(0)
      expect(atlasPages()).toBe(0)
      rerender(pane(true))
      send(0, snapshotFrame(`show ${i}`))
    }
    await frame()
    expect(ledger.liveGlContexts(), ledger.summary()).toBe(1)
    expect(ledger.liveGlObjects()).toBe(shown.objects)
    expect(ledger.textureBytes()).toBe(shown.texture)
    expect(atlasPages()).toBe(1)
  })

  it(`opening and closing Settings over it ${REPEAT}×: same`, async () => {
    await paintedPane()
    for (let i = 0; i < REPEAT; i++) {
      act(() => useSettingsStore.setState({ settingsOpen: true }))
      expect(ledger.liveGlContexts(), `settings open #${i}: ${ledger.summary()}`).toBe(0)
      act(() => useSettingsStore.setState({ settingsOpen: false }))
    }
    await frame()
    expect(ledger.liveGlContexts(), ledger.summary()).toBe(1)
    expect(atlasPages()).toBe(1)
  })

  it(`changing the font size ${REPEAT}×: one atlas page, old pages freed`, async () => {
    const { rerender } = await paintedPane()
    for (let i = 0; i < REPEAT; i++) {
      h.fontSize = 11 + (i % 8)
      rerender(pane(true))
      send(0, snapshotFrame(`font ${i}`))
    }
    await frame()
    expect(ledger.liveGlContexts()).toBe(1)
    expect(atlasPages()).toBe(1)
    const page = document.querySelector<HTMLCanvasElement>('[data-k2-atlas-page]')!
    expect(ledger.canvas2dBytes(), ledger.summary()).toBe(page.width * page.height * 4)
  })

  it(`opening and closing the tab ${REPEAT}×: nothing left behind`, async () => {
    for (let i = 0; i < REPEAT; i++) {
      const r = renderInRoom(room, pane(true))
      await connected(i + 1)
      send(i, snapshotFrame(`tab ${i}`))
      await frame()
      expect(glCanvas()).toBeTruthy()
      r.unmount()
    }
    expect(ledger.glContextsMade).toBe(REPEAT)
    expect(ledger.liveGlContexts(), ledger.summary()).toBe(0)
    expect(ledger.drawingBufferBytes()).toBe(0)
    expect(ledger.liveGlObjects()).toBe(0)
    expect(ledger.canvas2dBytes(), ledger.summary()).toBe(0)
    expect(atlasPages()).toBe(0)
  }, 60_000)

  it('many tabs, each shown once then parked: only the visible one holds a context', async () => {
    const TABS = 20
    const ids = Array.from({ length: TABS }, (_, i) => `tab-${i}`)
    const tree = (visibleIdx: number) => (
      <>
        {ids.map((id, i) => (
          <TabVisibilityContext.Provider key={id} value={i === visibleIdx}>
            <SessionViewChromeContext.Provider value={chrome}>
              <TerminalPane terminalId={id} cwd="/tmp/ws" />
            </SessionViewChromeContext.Provider>
          </TabVisibilityContext.Provider>
        ))}
      </>
    )
    const { rerender } = renderInRoom(room, tree(0))
    for (let i = 0; i < TABS; i++) {
      rerender(tree(i))
      // A pane dials its grid when first shown.
      await connected(i + 1)
      send(i, snapshotFrame(`tab ${i}`))
      await frame()
      expect(ledger.liveGlContexts(), `tab ${i} shown: ${ledger.summary()}`).toBe(1)
    }
    expect(ledger.glContextsMade).toBe(TABS)
    expect(atlasPages()).toBe(1)
  }, 60_000)
})
