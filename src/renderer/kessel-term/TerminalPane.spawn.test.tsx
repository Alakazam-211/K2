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
      recordOutput: vi.fn(),
      recordTitleActivity: vi.fn(),
      markSeen: vi.fn(),
      bindPaneAgentName: vi.fn(),
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
import { fakeScope } from '@/test-utils/fake-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useTabsStore } from '@/stores/tabs'
import { getDaemonWs } from '../kessel/daemon-ws'

// Home M3 — the pane's room: the mocked tabs store, an activity sink of
// spies (MS68), the primary scope.
function activitySpies() {
  return {
    recordOutput: vi.fn(),
    recordTitleActivity: vi.fn(),
    recordTitlePermission: vi.fn(),
    markSeen: vi.fn(),
    bindPaneAgentName: vi.fn(),
    bindPaneProject: vi.fn(),
  }
}
const room = testRoom({
  tabs: useTabsStore,
  activity: activitySpies(),
  presence: fixedStore({ roster: [], supported: true }),
})
import {
  contentBoxSize,
  FALLBACK_SPAWN_COLS,
  FALLBACK_SPAWN_ROWS,
  measurePaneFit,
} from './measurePaneFit'

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
 *  captured (URL + JSON body); the response satisfies TerminalPane's
 *  boot() contract.
 *
 *  Response cols/rows are daemon-echo placeholders only — they are NOT
 *  the client's spawn intent (request body is what we assert). Echoed
 *  here as 0 so readers do not mistake them for the old toy 120×40. */
function installFetchSpy(): {
  spawnCalls: () => number
  spawnBodies: () => Array<Record<string, unknown>>
} {
  const calls: string[] = []
  const bodies: Array<Record<string, unknown>> = []
  globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    if (url.includes('/cli/sessions/v2/spawn')) {
      calls.push(url)
      const raw = init?.body
      if (typeof raw === 'string') {
        bodies.push(JSON.parse(raw) as Record<string, unknown>)
      } else {
        bodies.push({})
      }
    }
    return {
      ok: true,
      status: 200,
      json: async () => ({
        sessionId: 'sess-test-1',
        agentName: 'tab-test',
        // Daemon-echo only — not client request intent.
        cols: 0,
        rows: 0,
        reused: false,
      }),
      text: async () => '',
    } as unknown as Response
  }) as unknown as typeof fetch
  return {
    spawnCalls: () => calls.length,
    spawnBodies: () => bodies,
  }
}

/**
 * Install geometry so font probe + content-box measurement work under
 * jsdom. Spawn uses {@link contentBoxSize} (clientWidth − padding),
 * matching ResizeObserver contentRect — not getBoundingClientRect.
 *
 * Pane defaults mirror TerminalPane padding `4px 0 0 4px` (top+left).
 */
function installGeometry(opts: {
  cellWidth: number
  cellHeight: number
  /** Content-box width/height (RO contentRect). */
  contentWidth: number
  contentHeight: number
  paddingLeft?: number
  paddingTop?: number
  paddingRight?: number
  paddingBottom?: number
}): () => void {
  const padL = opts.paddingLeft ?? 4
  const padT = opts.paddingTop ?? 4
  const padR = opts.paddingRight ?? 0
  const padB = opts.paddingBottom ?? 0
  // client* includes padding (border-box content+padding, no border).
  const clientW = opts.contentWidth + padL + padR
  const clientH = opts.contentHeight + padT + padB

  const originalGbr = HTMLElement.prototype.getBoundingClientRect
  HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
    // Font probe span: hidden absolute 'W' used by cell-metrics layout.
    const isProbe =
      this.tagName === 'SPAN' &&
      this.textContent === 'W' &&
      (this as HTMLElement).style?.visibility === 'hidden'
    if (isProbe) {
      return {
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        bottom: opts.cellHeight,
        right: opts.cellWidth,
        width: opts.cellWidth,
        height: opts.cellHeight,
        toJSON() {
          return this
        },
      } as DOMRect
    }
    // Border-box ≈ client size (no border). Deliberately NOT equal to
    // content box when padding > 0 — tests that use border-box for
    // measurePaneFit would drift by pad (the Issue 1 class of bug).
    return {
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      bottom: clientH,
      right: clientW,
      width: clientW,
      height: clientH,
      toJSON() {
        return this
      },
    } as DOMRect
  }

  const cwDesc = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')
  const chDesc = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight')
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get() {
      const isProbe =
        this.tagName === 'SPAN' &&
        this.textContent === 'W' &&
        (this as HTMLElement).style?.visibility === 'hidden'
      return isProbe ? opts.cellWidth : clientW
    },
  })
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get() {
      const isProbe =
        this.tagName === 'SPAN' &&
        this.textContent === 'W' &&
        (this as HTMLElement).style?.visibility === 'hidden'
      return isProbe ? opts.cellHeight : clientH
    },
  })

  const originalGcs = window.getComputedStyle.bind(window)
  window.getComputedStyle = ((el: Element, pseudo?: string | null) => {
    const base = originalGcs(el, pseudo ?? undefined)
    return new Proxy(base, {
      get(target, prop, receiver) {
        if (prop === 'paddingLeft') return `${padL}px`
        if (prop === 'paddingRight') return `${padR}px`
        if (prop === 'paddingTop') return `${padT}px`
        if (prop === 'paddingBottom') return `${padB}px`
        const v = Reflect.get(target, prop, receiver)
        return typeof v === 'function' ? (v as (...a: unknown[]) => unknown).bind(target) : v
      },
    }) as CSSStyleDeclaration
  }) as typeof window.getComputedStyle

  return () => {
    HTMLElement.prototype.getBoundingClientRect = originalGbr
    if (cwDesc) Object.defineProperty(HTMLElement.prototype, 'clientWidth', cwDesc)
    else delete (HTMLElement.prototype as unknown as { clientWidth?: unknown }).clientWidth
    if (chDesc) Object.defineProperty(HTMLElement.prototype, 'clientHeight', chDesc)
    else delete (HTMLElement.prototype as unknown as { clientHeight?: unknown }).clientHeight
    window.getComputedStyle = originalGcs
  }
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

describe('session_owned_elsewhere', () => {
  function installStatus(status: number, body: string): { urls: () => string[] } {
    const urls: string[] = []
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
      urls.push(String(input))
      return {
        ok: status >= 200 && status < 300,
        status,
        json: async () => ({}),
        text: async () => body,
      } as unknown as Response
    }) as unknown as typeof fetch
    return { urls: () => urls }
  }

  it('drops the pane and does not retry, error, or POST v2/close', async () => {
    const { urls } = installStatus(409, '{"error":"session_owned_elsewhere"}')
    renderInRoom(room, pane(true))
    await waitFor(() => expect(tabsApi.releasePaneOwnedElsewhere).toHaveBeenCalledWith('pg-test'))
    expect(document.body.textContent ?? '').not.toContain('Kessel:')
    expect(urls().filter((url) => url.includes('/cli/sessions/v2/spawn'))).toHaveLength(1)
    expect(urls().some((url) => url.includes('/cli/sessions/v2/close'))).toBe(false)
    expect(urls().some((url) => url.includes('clear_index'))).toBe(false)
    await settle()
    expect(urls().filter((url) => url.includes('/cli/sessions/v2/spawn'))).toHaveLength(1)
    expect(tabsApi.releasePaneOwnedElsewhere).toHaveBeenCalledTimes(1)
  })

  // V23 — the daemon refuses an empty-command respawn of a `tab-<pg>` that
  // was closed as a whole tab minutes ago (here or in another window). The
  // pane is dropped the same way: no retry, no close, no error strip.
  it('409 tab_closed drops the pane and does not retry, error, or POST v2/close', async () => {
    const { urls } = installStatus(409, '{"error":"tab_closed","agent_name":"tab-pg-test"}')
    renderInRoom(room, pane(true))
    await waitFor(() => expect(tabsApi.releasePaneOwnedElsewhere).toHaveBeenCalledWith('pg-test'))
    expect(document.body.textContent ?? '').not.toContain('Kessel:')
    expect(document.body.textContent ?? '').not.toContain('spawn 409')
    expect(urls().filter((url) => url.includes('/cli/sessions/v2/spawn'))).toHaveLength(1)
    expect(urls().some((url) => url.includes('/cli/sessions/v2/close'))).toBe(false)
    await settle()
    expect(urls().filter((url) => url.includes('/cli/sessions/v2/spawn'))).toHaveLength(1)
    expect(tabsApi.releasePaneOwnedElsewhere).toHaveBeenCalledTimes(1)
  })

  it('other 4xx still surfaces the spawn error and does not drop the pane', async () => {
    installStatus(400, '{"error":"bad request"}')
    renderInRoom(room, pane(true))
    await waitFor(() => expect(document.body.textContent ?? '').toContain('spawn 400'))
    expect(tabsApi.releasePaneOwnedElsewhere).not.toHaveBeenCalled()
    expect(document.body.textContent ?? '').toContain('bad request')
  })
})

describe("a pane in another server's room (Home M3)", () => {
  it("spawns with that room's scope and drops the pane from that room's tabs, never the primary's", async () => {
    const B = fakeScope('b.test')
    const releaseInB = vi.fn()
    const roomB = testRoom({
      presence: fixedStore({ roster: [], supported: true }),
      key: 'b.test|p1:w1',
      isPrimary: false,
      localCommands: false,
      scope: B,
      tabs: {
        getState: () => ({
          setTerminalSandboxBackend: vi.fn(),
          setTerminalConversationId: vi.fn(),
          setTabTitle: vi.fn(),
          tabs: [],
          extraGroups: [],
          releasePaneOwnedElsewhere: releaseInB,
        }),
      },
      activity: activitySpies(),
    })
    globalThis.fetch = vi.fn(async () => ({
      ok: false,
      status: 409,
      json: async () => ({}),
      text: async () => '{"error":"session_owned_elsewhere"}',
    })) as unknown as typeof fetch

    renderInRoom(roomB, pane(true))
    await waitFor(() => expect(releaseInB).toHaveBeenCalledWith('pg-test'))
    expect(tabsApi.releasePaneOwnedElsewhere).not.toHaveBeenCalled()
    const scopes = vi.mocked(getDaemonWs).mock.calls.map((c) => c[0])
    expect(scopes.length).toBeGreaterThan(0)
    expect(scopes.every((s) => s === B)).toBe(true)
  })
})

describe('lazy spawn — restored never-attached bare tabs', () => {
  it('a hidden bare tab (no command, no session) does NOT spawn on mount', async () => {
    const { spawnCalls } = installFetchSpy()
    renderInRoom(room, pane(false))
    await settle()
    expect(spawnCalls()).toBe(0)
  })

  it('the visible (active) tab spawns on mount', async () => {
    const { spawnCalls } = installFetchSpy()
    renderInRoom(room, pane(true))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('a hidden tab WITH a resumable sessionId spawns on mount (stays warm)', async () => {
    const { spawnCalls } = installFetchSpy()
    renderInRoom(room, pane(false, { sessionId: 'claude-session-uuid' }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('a hidden tab with a real command spawns on mount (background work)', async () => {
    const { spawnCalls } = installFetchSpy()
    renderInRoom(room, pane(false, { command: 'claude', args: ['--resume', 'abc'] }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('a hidden tab attaching to an existing daemon session spawns on mount', async () => {
    const { spawnCalls } = installFetchSpy()
    renderInRoom(room, pane(false, { attachAgentName: 'proj-uuid' }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
  })

  it('becoming visible fires the deferred spawn exactly once; later flips never re-spawn', async () => {
    const { spawnCalls } = installFetchSpy()
    const view = renderInRoom(room, pane(false))
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

describe('measure-first spawn body cols/rows', () => {
  it('POSTs measured content-box fit when pane is measurable (not 120×40)', async () => {
    // content 800×640 with pad 4 top/left — RO contentRect path.
    const restoreGeo = installGeometry({
      cellWidth: 8,
      cellHeight: 16,
      contentWidth: 800,
      contentHeight: 640,
    })
    try {
      const expected = measurePaneFit({ width: 800, height: 640 }, 8, 16)
      expect(expected).not.toBeNull()
      expect(expected).not.toEqual({ cols: 120, rows: 40 })

      const { spawnCalls, spawnBodies } = installFetchSpy()
      renderInRoom(room, pane(true))
      await waitFor(() => expect(spawnCalls()).toBe(1))

      const body = spawnBodies()[0]
      expect(body).toBeDefined()
      expect(body.cols).toBe(expected!.cols)
      expect(body.rows).toBe(expected!.rows)
      expect(body.cols).not.toBe(120)
      expect(body.rows).not.toBe(40)
    } finally {
      restoreGeo()
    }
  })

  it('spawn cols/rows match RO contentRect path (content-box parity, not border-box)', async () => {
    // contentW % cellW ∈ {0,1,2,3} is the Issue 1 trap: border-box
    // (content+pad) floored after −4 yields one extra col vs content-box.
    const cellW = 8
    const cellH = 16
    const contentW = 800 // 800 % 8 === 0 → border-box path would be cols+1
    const contentH = 640
    const padL = 4
    const padT = 4
    const restoreGeo = installGeometry({
      cellWidth: cellW,
      cellHeight: cellH,
      contentWidth: contentW,
      contentHeight: contentH,
      paddingLeft: padL,
      paddingTop: padT,
    })
    try {
      const roFit = measurePaneFit({ width: contentW, height: contentH }, cellW, cellH)
      // What the old (buggy) border-box path would compute:
      const borderW = contentW + padL
      const borderH = contentH + padT
      const borderFit = measurePaneFit({ width: borderW, height: borderH }, cellW, cellH)
      expect(roFit).not.toBeNull()
      expect(borderFit).not.toBeNull()
      // Prove the sizes actually diverge for this fixture — otherwise
      // the parity test would not catch a regression to border-box.
      expect(borderFit!.cols).not.toBe(roFit!.cols)

      const { spawnCalls, spawnBodies } = installFetchSpy()
      renderInRoom(room, pane(true))
      await waitFor(() => expect(spawnCalls()).toBe(1))

      const body = spawnBodies()[0]
      // Must match content-box / RO, not border-box.
      expect(body.cols).toBe(roFit!.cols)
      expect(body.rows).toBe(roFit!.rows)
      expect(body.cols).not.toBe(borderFit!.cols)

      // contentBoxSize on the mounted pane agrees with the fixture.
      const el = document.querySelector('[data-terminal-container]') as HTMLElement | null
      expect(el).not.toBeNull()
      expect(contentBoxSize(el)).toEqual({ width: contentW, height: contentH })
    } finally {
      restoreGeo()
    }
  })

  it('zero-size container uses FALLBACK_SPAWN (80×24), not toy 120×40, and does not crash', async () => {
    const restoreGeo = installGeometry({
      cellWidth: 8,
      cellHeight: 16,
      contentWidth: 0,
      contentHeight: 0,
    })
    try {
      const { spawnCalls, spawnBodies } = installFetchSpy()
      renderInRoom(room, pane(true))
      await waitFor(() => expect(spawnCalls()).toBe(1))

      const body = spawnBodies()[0]
      expect(body).toBeDefined()
      expect(body.cols).toBe(FALLBACK_SPAWN_COLS)
      expect(body.rows).toBe(FALLBACK_SPAWN_ROWS)
      expect(body.cols).toBe(80)
      expect(body.rows).toBe(24)
      expect(body.cols).not.toBe(120)
      expect(body.rows).not.toBe(40)
    } finally {
      restoreGeo()
    }
  })
})

describe('named chat spawn seed+lock', () => {
  it('POSTs display name as label and label_locked, not a harness basename', async () => {
    const { spawnCalls, spawnBodies } = installFetchSpy()
    renderInRoom(room, pane(true, {
      command: 'claude',
      args: ['--resume', '01920000-aaaa-7000-8000-000000000001'],
      seedLabel: 'Code Review',
      lockLabel: true,
    }))
    await waitFor(() => expect(spawnCalls()).toBe(1))
    const body = spawnBodies()[0]
    expect(body.label).toBe('Code Review')
    expect(body.label_locked).toBe(true)
    expect(body.label).not.toBe('claude')
  })
})

// Home M5 — released daemons up to 0.41.6 ignore `attach_only` and would
// SPAWN a tab that is not running. A view-only room on a server that does
// not report `spawn-attach-only` sends the spawn only for a session that
// server lists as live; otherwise it shows "Not running on …" and sends
// nothing. A usable room is unchanged.
describe('a view-only room on a server without spawn-attach-only (released ≤ 0.41.6)', () => {
  function roomOn(opts: { readOnly: boolean; attachOnly: boolean }) {
    const base = fakeScope('old-b.test')
    const scope = {
      ...base,
      serverSupports: (f: string) => (f === 'spawn-attach-only' ? opts.attachOnly : true),
    } as typeof base
    return testRoom({
      presence: fixedStore({ roster: [], supported: true }),
      key: 'old-b.test|p1:w1',
      isPrimary: false,
      localCommands: false,
      readOnly: opts.readOnly,
      scope,
      cwd: '/srv/ws',
      tabs: {
        getState: () => ({
          setTerminalSandboxBackend: vi.fn(),
          setTerminalConversationId: vi.fn(),
          setTabTitle: vi.fn(),
          tabs: [],
          extraGroups: [],
          releasePaneOwnedElsewhere: vi.fn(),
        }),
      },
      activity: activitySpies(),
    })
  }

  function liveOnB(agentNames: string[]): void {
    vi.mocked(daemonCliGet).mockImplementation(async (_scope, route) => {
      if (route !== 'sessions/list-for-workspace') throw new Error(`unexpected GET ${route}`)
      return agentNames.map((agentName) => ({ agentName, sessionId: `s-${agentName}` }))
    })
  }

  function listCalls(): Array<{ scopeHost: string; params: unknown }> {
    return vi
      .mocked(daemonCliGet)
      .mock.calls.filter((c) => c[1] === 'sessions/list-for-workspace')
      .map((c) => ({ scopeHost: (c[0] as { hostKey: string }).hostKey, params: c[2] }))
  }

  it('never issues a spawn for a tab that is not live there, and says Not running', async () => {
    const { spawnCalls } = installFetchSpy()
    liveOnB(['tab-someone-else'])
    renderInRoom(roomOn({ readOnly: true, attachOnly: false }), pane(true))
    await waitFor(() => expect(document.body.textContent ?? '').toContain('Not running on old-b.test'))
    await settle()
    expect(spawnCalls()).toBe(0)
    expect(listCalls()).toEqual([{ scopeHost: 'old-b.test', params: { path: '/srv/ws' } }])
  })

  it('a hidden tab with a sessionId (eager) that is not live still sends nothing', async () => {
    const { spawnCalls } = installFetchSpy()
    liveOnB([])
    renderInRoom(roomOn({ readOnly: true, attachOnly: false }), pane(false, { sessionId: 'gone-1' }))
    await waitFor(() => expect(listCalls().length).toBe(1))
    await settle()
    expect(spawnCalls()).toBe(0)
  })

  it('attaches (one spawn, attach_only) when that server lists the tab live', async () => {
    const { spawnCalls, spawnBodies } = installFetchSpy()
    liveOnB(['tab-pg-test'])
    renderInRoom(roomOn({ readOnly: true, attachOnly: false }), pane(true))
    await waitFor(() => expect(spawnCalls()).toBe(1))
    expect(spawnBodies()[0].agent_name).toBe('tab-pg-test')
    expect(spawnBodies()[0].attach_only).toBe(true)
  })

  it('a failed live list is an error, never a spawn', async () => {
    const { spawnCalls } = installFetchSpy()
    vi.mocked(daemonCliGet).mockImplementation(async () => {
      throw new Error('503 starting')
    })
    renderInRoom(roomOn({ readOnly: true, attachOnly: false }), pane(true))
    await waitFor(() => expect(document.body.textContent ?? '').toContain('Could not read the sessions on old-b.test'))
    await settle()
    expect(spawnCalls()).toBe(0)
  })

  it('a server that reports spawn-attach-only gets the attach_only spawn with no list read (M4 path)', async () => {
    const { spawnCalls, spawnBodies } = installFetchSpy()
    liveOnB([])
    renderInRoom(roomOn({ readOnly: true, attachOnly: true }), pane(true))
    await waitFor(() => expect(spawnCalls()).toBe(1))
    expect(spawnBodies()[0].attach_only).toBe(true)
    expect(listCalls()).toEqual([])
  })

  it('a usable room spawns as before: no list read, no attach_only (M5 path unchanged)', async () => {
    const { spawnCalls, spawnBodies } = installFetchSpy()
    liveOnB([])
    renderInRoom(roomOn({ readOnly: false, attachOnly: false }), pane(true))
    await waitFor(() => expect(spawnCalls()).toBe(1))
    expect(spawnBodies()[0].attach_only).toBe(undefined)
    expect(listCalls()).toEqual([])
  })
})
