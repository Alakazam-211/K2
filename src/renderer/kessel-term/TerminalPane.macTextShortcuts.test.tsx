// @vitest-environment jsdom
// Settings → Terminal → Mac text shortcuts reaches Kessel terminals: on,
// ⌥← / ⌘← and friends are sent as the readline keys (word back, line
// start); off, the pane sends the plain key sequence instead. The real
// TerminalPane under jsdom; only its I/O boundaries are mocked (the same
// set as TerminalPane.threadView.test.tsx), and the socket records what the
// pane sends.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, waitFor } from '@testing-library/react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'

const h = vi.hoisted(() => ({
  sockets: [] as Array<{ sent: string[]; onmessage: ((e: { data: string }) => void) | null }>,
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
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => undefined),
  daemonCliPost: vi.fn(async () => undefined),
}))
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
vi.mock('@/lib/handle-remote-drop', () => ({ executeRemoteDrop: vi.fn(async () => undefined) }))
vi.mock('@/components/Terminal/TerminalComposeBar', () => ({ TerminalComposeBar: () => null }))
vi.mock('@/stores/terminal-settings', () => {
  const state = { fontSize: 13, linkClickMode: 'click', painter: 'dom', openLinksInSplitPane: false }
  return {
    useTerminalSettingsStore: Object.assign((sel: (s: typeof state) => unknown) => sel(state), { getState: () => state }),
  }
})
vi.mock('@/stores/tabs', () => ({
  useTabsStore: {
    getState: () => ({
      setTerminalSandboxBackend: vi.fn(),
      setTerminalConversationId: vi.fn(),
      setTabTitle: vi.fn(),
      tabs: [],
      extraGroups: [],
      releasePaneOwnedElsewhere: vi.fn(),
    }),
  },
}))
vi.mock('@/stores/window-focus', () => ({
  useWindowFocusStore: { getState: () => ({ isFocused: true }), subscribe: () => () => undefined },
}))
vi.mock('@/stores/session-labels', () => ({
  useSessionLabelsStore: { getState: () => ({ setSessionLabel: vi.fn() }) },
}))
vi.mock('@/stores/active-agents', () => ({
  useActiveAgentsStore: { getState: () => ({ agents: new Map() }) },
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
  onActiveHostChange: () => () => {},
}))

import { TerminalPane } from './TerminalPane'
import { fixedStore, renderInRoom, testRoom } from '@/test-utils/room'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'

const room = testRoom({
  tabs: useTabsStore,
  activity: { setViewing: vi.fn() },
  presence: fixedStore({ roster: [], supported: true }),
})

class StubResizeObserver {
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
  close(): void {
    this.readyState = 3
  }
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
  h.sockets = []
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

/** Mount a pane and feed it a snapshot so it takes keys. */
async function readyPane(): Promise<HTMLTextAreaElement> {
  renderInRoom(
    room,
    <TabVisibilityContext.Provider value={true}>
      <TerminalPane terminalId="pg-mac-keys" cwd="/tmp/ws" />
    </TabVisibilityContext.Provider>,
  )
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
  return document.querySelector('textarea') as HTMLTextAreaElement
}

/** The text of every input frame the pane sent. */
function typed(): string[] {
  return h.sockets[0].sent
    .map((x) => JSON.parse(x) as { action?: string; text?: string })
    .filter((m) => m.action === 'input')
    .map((m) => m.text ?? '')
}

function setMacShortcuts(on: boolean): void {
  useSettingsStore.setState((s) => ({ terminal: { ...s.terminal, naturalTextEditing: on } }))
}

describe('Mac text shortcuts in Kessel terminals', () => {
  it('on: ⌥← moves back a word and ⌘← goes to the line start', async () => {
    setMacShortcuts(true)
    const input = await readyPane()
    fireEvent.keyDown(input, { key: 'ArrowLeft', code: 'ArrowLeft', altKey: true })
    fireEvent.keyDown(input, { key: 'ArrowLeft', code: 'ArrowLeft', metaKey: true })
    expect(typed()).toEqual(['\x1bb', '\x01'])
  })

  it('off: the same keys are sent as plain keys, never the Mac mapping', async () => {
    setMacShortcuts(false)
    const input = await readyPane()
    fireEvent.keyDown(input, { key: 'ArrowLeft', code: 'ArrowLeft', altKey: true })
    fireEvent.keyDown(input, { key: 'ArrowLeft', code: 'ArrowLeft', metaKey: true })
    const sent = typed()
    expect(sent).not.toContain('\x1bb')
    expect(sent).not.toContain('\x01')
    // ⌥← still reaches the shell, as the plain modified arrow.
    expect(sent.length).toBeGreaterThan(0)
  })

  it('a change takes effect in an open terminal, without reopening it', async () => {
    setMacShortcuts(true)
    const input = await readyPane()
    fireEvent.keyDown(input, { key: 'ArrowLeft', code: 'ArrowLeft', altKey: true })
    act(() => setMacShortcuts(false))
    fireEvent.keyDown(input, { key: 'ArrowLeft', code: 'ArrowLeft', altKey: true })
    const sent = typed()
    expect(sent[0]).toBe('\x1bb')
    expect(sent.slice(1)).not.toContain('\x1bb')
    expect(sent.length).toBe(2)
  })
})
