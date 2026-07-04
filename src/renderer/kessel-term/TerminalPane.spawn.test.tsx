// @vitest-environment jsdom
//
// 2026-07-03 — lazy spawn for restored never-attached bare tabs (the
// workspace-switch latency fix).
//
// A workspace mount renders EVERY saved tab's pane (retained-view
// model); pre-fix each TerminalPane fired POST /cli/sessions/v2/spawn
// on mount, so a restored layout with N bare tabs cost N sequential
// round-trips per workspace entry (each refused by the daemon's
// bare-tab cap — pure latency). These tests pin the gate:
//
//   - hidden + bare (no command / sessionId / attachAgentName)
//       → NO spawn POST on mount;
//   - visible → spawns (the active tab is always warm);
//   - hidden + sessionId (resumable / live session known to the
//     client) → spawns (stays warm);
//   - hidden + command (real program, e.g. background heartbeat
//     spawn) → spawns;
//   - hidden + attachAgentName (existing daemon session) → spawns;
//   - hidden bare that BECOMES visible → the deferred spawn fires
//     exactly once, and later visibility flips never re-issue it
//     (the 0.39.13 stable-deps guarantee).
//
// The REAL TerminalPane mounts under jsdom; only its I/O boundaries
// are mocked (fetch, WebSocket, daemon creds, Tauri invoke, stores).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, cleanup, waitFor } from '@testing-library/react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'

// ── I/O boundary mocks ────────────────────────────────────────────────────

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
    painter: 'dom',
    openLinksInSplitPane: false,
  }
  return {
    useTerminalSettingsStore: Object.assign(
      (sel: (s: typeof state) => unknown) => sel(state),
      { getState: () => state },
    ),
  }
})
vi.mock('@/stores/tabs', () => ({
  useTabsStore: {
    getState: () => ({
      setTerminalSandboxBackend: vi.fn(),
      setTabTitle: vi.fn(),
      tabs: [],
      extraGroups: [],
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
      recordOutput: vi.fn(),
      recordTitleActivity: vi.fn(),
      markSeen: vi.fn(),
      agents: new Map(),
    }),
  },
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: {
    getState: () => ({ activeHost: 'local' }),
  },
}))

import { TerminalPane } from './TerminalPane'

// ── Global stubs (jsdom gaps) ─────────────────────────────────────────────

class StubResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** WebSocket that never connects — the grid-WS handshake promise stays
 *  pending, which is fine: these tests end at the spawn POST. */
class StubWebSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  url: string
  binaryType = 'blob'
  readyState = 0
  onopen: (() => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null
  onmessage: (() => void) | null = null
  constructor(url: string) {
    this.url = url
  }
  send(): void {}
  close(): void {
    this.readyState = 3
  }
}

/** Spawn-recording fetch. Every POST to /cli/sessions/v2/spawn is
 *  captured; the response satisfies TerminalPane's boot() contract. */
function installFetchSpy(): { spawnCalls: () => number } {
  const calls: string[] = []
  globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input)
    if (url.includes('/cli/sessions/v2/spawn')) calls.push(url)
    return {
      ok: true,
      status: 200,
      json: async () => ({
        sessionId: 'sess-test-1',
        agentName: 'tab-test',
        cols: 120,
        rows: 40,
        reused: false,
      }),
      text: async () => '',
    } as unknown as Response
  }) as unknown as typeof fetch
  return { spawnCalls: () => calls.length }
}

/** Deterministic settle: flush the microtask queue through enough turns
 *  for boot()'s await chain (creds → fetch → json) to have run if it was
 *  going to. Used for NEGATIVE assertions, where waitFor can't help. */
async function settle(turns = 20): Promise<void> {
  for (let i = 0; i < turns; i += 1) {
    await new Promise((r) => setTimeout(r, 0))
  }
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', StubResizeObserver)
  vi.stubGlobal('WebSocket', StubWebSocket)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

function pane(visible: boolean, props: Partial<React.ComponentProps<typeof TerminalPane>> = {}) {
  return (
    <TabVisibilityContext.Provider value={visible}>
      <TerminalPane terminalId="pg-test" cwd="/tmp/ws" {...props} />
    </TabVisibilityContext.Provider>
  )
}

describe('lazy spawn — restored never-attached bare tabs', () => {
  it('a hidden bare tab (no command, no session) does NOT spawn on mount', async () => {
    const { spawnCalls } = installFetchSpy()
    render(pane(false))
    await settle()
    expect(spawnCalls()).toBe(0)
  })

  it('the visible (active) tab spawns on mount', async () => {
    const { spawnCalls } = installFetchSpy()
    render(pane(true))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('a hidden tab WITH a resumable sessionId spawns on mount (stays warm)', async () => {
    const { spawnCalls } = installFetchSpy()
    render(pane(false, { sessionId: 'claude-session-uuid' }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('a hidden tab with a real command spawns on mount (background work)', async () => {
    const { spawnCalls } = installFetchSpy()
    render(pane(false, { command: 'claude', args: ['--resume', 'abc'] }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('a hidden tab attaching to an existing daemon session spawns on mount', async () => {
    const { spawnCalls } = installFetchSpy()
    render(pane(false, { attachAgentName: 'proj-uuid' }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('becoming visible fires the deferred spawn exactly once; later flips never re-spawn', async () => {
    const { spawnCalls } = installFetchSpy()
    const view = render(pane(false))
    await settle()
    expect(spawnCalls()).toBe(0)

    // First reveal → the one deferred spawn.
    view.rerender(pane(true))
    await waitFor(() => expect(spawnCalls()).toBe(1))

    // Hide and reveal again — arming is one-way; the spawn effect's
    // stable deps must not re-fire on visibility churn.
    view.rerender(pane(false))
    await settle()
    view.rerender(pane(true))
    await settle()
    expect(spawnCalls()).toBe(1)
  })
})
