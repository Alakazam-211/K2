// @vitest-environment jsdom
// Thread view inside TerminalPane (prd-thread-view-focus-v1): what the pane
// hands its Thread in each view. The real TerminalPane under jsdom; only its
// I/O boundaries are mocked (same set as TerminalPane.spawn.test.tsx).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, act } from '@testing-library/react'
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
  TerminalComposeBar: () => null,
}))

// Stores — selector-hook and/or getState() shapes, matching how
// TerminalPane consumes each one.
vi.mock('@/stores/terminal-settings', () => {
  const state = {
    fontSize: 13,
    linkClickMode: 'click',
    painter: h.painter,
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
  useSettingsStore.setState({ threadTextWidth: THREAD_TEXT_WIDTH_DEFAULT, settingsOpen: false })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('Thread text limit in TerminalPane', () => {
  it('Thread view passes the limit', () => {
    renderInRoom(room, pane(chrome({ viewTab: 'thread' })))
    expect(h.threadProps.at(-1)?.textCapPx).toBe(720)
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
