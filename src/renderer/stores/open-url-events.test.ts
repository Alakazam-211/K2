// Browser-pane arc (0.40.34) — daemon-routed `open_url` → embedded browser tab.
//
// The daemon emits APP-LEVEL `{ kind: 'open_url', url, source }` on the
// /cli/sessions/events bus when a session's `k2 open <url>` shim or a
// terminal hyperlink click routes an http(s) target through
// /cli/fs/open-external (the daemon SKIPS its own `open`/`xdg-open` when a
// session-events subscriber exists — so WITHOUT this renderer wiring,
// clicking a URL does nothing). This suite pins, against the REAL
// session-events module (fake WebSocket at the boundary) + the REAL
// in-memory tabs store:
//   - `onOpenUrl` registration: an `open_url` frame on the app-level socket
//     fires the callback with the unwrapped `(url, source)` pair,
//   - the returned unsubscribe fn deregisters (no further callbacks),
//   - the app-scope consumer (`initOpenUrlBrowserTabs`) surfaces the URL as
//     a NEW browser tab via `openUrlInNewTab`, and ignores blank/empty urls.
//
// vitest env is `node` (no Tauri / no real WebSocket). The daemon/Tauri
// boundaries the tabs module graph touches are mocked so importing it is
// inert; `WebSocket` is a recording fake so a test can push frames through
// the real `subscribeToActiveState` onmessage → dispatch path.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ── localStorage (real, in-memory) so the tabs module graph's persist paths run ─
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'resolved-agent'),
  setChatSession: vi.fn(async () => undefined),
}))
// Real session-events runs on top of this transport boundary — creds
// resolve so `subscribeToActiveState` constructs the (fake) WebSocket.
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))
// Keep the hello-driven `refreshActiveSnapshot` inert (capability off) —
// this suite owns the open_url path only.
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => false),
  FEATURES: {},
}))

// ── Fake WebSocket (recording) ────────────────────────────────────────────
class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  url: string
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: ((ev: { code: number; reason?: string }) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  close(): void {
    // No onclose echo — the unsubscribe path must not depend on it.
  }
}
vi.stubGlobal('WebSocket', FakeWebSocket)

import { subscribeToActiveState, onOpenUrl, type UnsubscribeFn } from './session-events'
import { useTabsStore, initOpenUrlBrowserTabs, type BrowserItemData } from './tabs'

/** Open the app-level subscription and wait for the fake socket. */
async function openAppSocket(): Promise<{ ws: FakeWebSocket; unsub: UnsubscribeFn }> {
  const unsub = subscribeToActiveState()
  await vi.waitFor(() => {
    expect(FakeWebSocket.instances.length).toBeGreaterThan(0)
  })
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  ws.onopen?.()
  return { ws, unsub }
}

function pushFrame(ws: FakeWebSocket, frame: unknown): void {
  ws.onmessage?.({ data: JSON.stringify(frame) })
}

/** Every browser item across all tabs, as (tabTitle, url) pairs. */
function browserItems(): Array<{ title: string; url: string }> {
  const out: Array<{ title: string; url: string }> = []
  for (const tab of useTabsStore.getState().tabs) {
    for (const pg of tab.paneGroups.values()) {
      for (const item of pg.items) {
        if (item.type === 'browser') {
          out.push({ title: tab.title, url: (item.data as BrowserItemData).url })
        }
      }
    }
  }
  return out
}

const cleanups: UnsubscribeFn[] = []

beforeEach(() => {
  FakeWebSocket.instances = []
  mem.clear()
  useTabsStore.setState({ tabs: [], activeTabId: null })
})

afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()!()
})

describe('onOpenUrl (app-level open_url dispatch)', () => {
  it('fires the registered callback with (url, source) when an open_url frame arrives', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const calls: Array<[string, string]> = []
    cleanups.push(onOpenUrl((url, source) => calls.push([url, source])))

    pushFrame(ws, { kind: 'open_url', url: 'https://example.com/docs', source: 'shim' })
    expect(calls).toEqual([['https://example.com/docs', 'shim']])

    pushFrame(ws, { kind: 'open_url', url: 'https://k2.dev', source: 'terminal-link' })
    expect(calls).toEqual([
      ['https://example.com/docs', 'shim'],
      ['https://k2.dev', 'terminal-link'],
    ])
  })

  it('stops firing after the returned unsubscribe fn runs', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const calls: string[] = []
    const off = onOpenUrl((url) => calls.push(url))

    pushFrame(ws, { kind: 'open_url', url: 'https://one.example', source: 'shim' })
    expect(calls).toEqual(['https://one.example'])

    off()
    pushFrame(ws, { kind: 'open_url', url: 'https://two.example', source: 'shim' })
    expect(calls).toEqual(['https://one.example'])
  })
})

describe('initOpenUrlBrowserTabs (app-scope consumer)', () => {
  it('surfaces an open_url frame as a NEW browser tab via openUrlInNewTab', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)
    cleanups.push(initOpenUrlBrowserTabs())

    expect(useTabsStore.getState().tabs).toHaveLength(0)
    pushFrame(ws, { kind: 'open_url', url: 'https://example.com/page', source: 'terminal-link' })

    const items = browserItems()
    expect(items).toHaveLength(1)
    expect(items[0].url).toBe('https://example.com/page')
    // openUrlInNewTab activates the new tab (deliberate: a URL the user
    // clicked should come to front).
    const { tabs, activeTabId } = useTabsStore.getState()
    expect(tabs).toHaveLength(1)
    expect(activeTabId).toBe(tabs[0].id)
  })

  it('ignores blank/empty urls (no tab minted)', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)
    cleanups.push(initOpenUrlBrowserTabs())

    pushFrame(ws, { kind: 'open_url', url: '', source: 'shim' })
    pushFrame(ws, { kind: 'open_url', url: '   ', source: 'shim' })
    expect(useTabsStore.getState().tabs).toHaveLength(0)

    // Sanity: a real URL after the blanks still opens (the consumer stayed
    // registered — the blank guard returns early, it doesn't unsubscribe).
    pushFrame(ws, { kind: 'open_url', url: 'https://still.works', source: 'shim' })
    expect(browserItems()).toEqual([
      expect.objectContaining({ url: 'https://still.works' }),
    ])
  })

  it('mints one tab per event (no dedupe by design)', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)
    cleanups.push(initOpenUrlBrowserTabs())

    pushFrame(ws, { kind: 'open_url', url: 'https://same.example', source: 'shim' })
    pushFrame(ws, { kind: 'open_url', url: 'https://same.example', source: 'terminal-link' })
    expect(useTabsStore.getState().tabs).toHaveLength(2)
  })
})
