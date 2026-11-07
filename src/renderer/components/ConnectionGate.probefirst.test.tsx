// @vitest-environment jsdom
// 0.46.10 probe-first recovery, through the REAL ConnectionGate and the real
// `daemonCliPost` → `cliFetch` path.
//
// The field bug: adding a workspace on a remote server fired a best-effort
// `agents/run-workspace-ingest` POST; WebKit lost it ("Load failed",
// reported as CORS). That one failed request flipped the healthy host into
// recovery / an auto-reload: every socket dropped and the view reset.
//
// Now a failed request is evidence that asks the gate to probe:
//   - healthy probe → recovery stays 'connected', no reload;
//   - failed probe  → 'reconnecting' at once (a dead host is still caught).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => {
  // Stores evaluated on import fetch at once; nothing here reaches a network.
  globalThis.fetch = (async () => new Response('{}', { status: 200 })) as typeof fetch
  return {
    app: { mounts: 0 },
    bootOk: true,
    bootFetches: 0,
    arbiter: null as null | { status: number; body: string },
    arbiterCalls: 0,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'remote_boot_probe') {
      h.arbiterCalls += 1
      if (h.arbiter === null) throw new Error('arbiter: unreachable')
      return h.arbiter
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '9.9.9' }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/components/TopBar/GateChrome', () => ({ default: () => null }))
vi.mock('@/App', async () => {
  const React = await import('react')
  function App(): React.JSX.Element {
    React.useEffect(() => {
      h.app.mounts += 1
    }, [])
    return <div data-stub-app="" />
  }
  return { default: App, HomeRoomsHost: () => null, ZenHost: () => null }
})

import { cleanup, render, waitFor } from '@testing-library/react'
import { ConnectionGate } from './ConnectionGate'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { __resetForceProbeLatchForTests } from '@/lib/connection-gate-probe'

const C: ConnectHost = {
  id: 'id-c', label: 'Box C', hostname: 'c.k2.dev', port: 443, secure: true,
  token: 'tok-c', remember: false, lastConnectedAt: null,
} as ConnectHost

const READY = JSON.stringify({ version: '9.9.9', protocol: 1, phase: 'ready', detail: '' })
const RELOAD_FLAG = 'k2.wedge-reloaded:c.k2.dev'

beforeEach(() => {
  h.app.mounts = 0
  h.bootOk = true
  h.bootFetches = 0
  // The out-of-webview arbiter sees the host READY: under the old rule a
  // single webview miss would have auto-reloaded the page here.
  h.arbiter = { status: 200, body: READY }
  h.arbiterCalls = 0
  sessionStorage.clear()
  __resetForceProbeLatchForTests()
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
  vi.stubGlobal('fetch', async (input: RequestInfo | URL) => {
    const url = String(input)
    if (url.includes('/boot-status')) {
      h.bootFetches += 1
      if (!h.bootOk) throw new TypeError('Load failed')
      return new Response(READY, { status: 200 })
    }
    if (url.includes('/cli/agents/run-workspace-ingest')) {
      // What WebKit hands JS for a dropped preflight / connection.
      throw new TypeError('Load failed')
    }
    return new Response('{}', { status: 200 })
  })
  useConnectHostStore.setState({
    hosts: [C],
    activeHost: C,
    recovery: { kind: 'connected' },
    hydrateFromDisk: async () => undefined,
  } as never)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  __resetForceProbeLatchForTests()
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], recovery: { kind: 'connected' } } as never)
})

async function mountedGate(): Promise<void> {
  render(<ConnectionGate />)
  await waitFor(() => expect(h.app.mounts).toBe(1), { timeout: 5_000 })
  expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
}

async function failIngest(): Promise<void> {
  await expect(
    daemonCliPost(primaryScope(), 'agents/run-workspace-ingest', { project_path: '/srv/new' }),
  ).rejects.toThrow('Load failed')
}

describe('probe-first recovery (0.46.10)', () => {
  it('one load-failed POST on a healthy host: the gate probes, stays connected, never reloads', async () => {
    await mountedGate()
    const before = h.bootFetches

    await failIngest()

    // The failed request asked for a probe, and it answered.
    await waitFor(() => expect(h.bootFetches).toBeGreaterThan(before), { timeout: 2_000 })
    // Let the tick finish (accept branch) before judging.
    await new Promise((r) => setTimeout(r, 50))
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
    expect(useConnectHostStore.getState().connectionStatus).toBe('connected')
    expect(sessionStorage.getItem(RELOAD_FLAG)).toBe(null)
    expect(h.arbiterCalls).toBe(0)
    expect(h.app.mounts).toBe(1)
  })

  it('the same failed POST while the probe fails too: recovery at once (dead host still caught)', async () => {
    await mountedGate()
    h.bootOk = false
    // A dead host: the fresh out-of-webview socket can't reach it either.
    h.arbiter = null

    await failIngest()

    await waitFor(
      () => expect(useConnectHostStore.getState().recovery.kind).toBe('reconnecting'),
      { timeout: 2_000 },
    )
    // The probe was retried once on a fresh socket before it counted.
    expect(h.bootFetches).toBeGreaterThanOrEqual(3)
    expect(sessionStorage.getItem(RELOAD_FLAG)).toBe(null)
  })

  it('a probe that misses once and then answers (dead pooled socket) does not count as a failure', async () => {
    await mountedGate()
    let misses = 1
    vi.stubGlobal('fetch', async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes('/boot-status')) {
        h.bootFetches += 1
        if (misses > 0) {
          misses -= 1
          throw new TypeError('Load failed')
        }
        return new Response(READY, { status: 200 })
      }
      if (url.includes('/cli/agents/run-workspace-ingest')) throw new TypeError('Load failed')
      return new Response('{}', { status: 200 })
    })
    const before = h.bootFetches

    await failIngest()

    await waitFor(() => expect(h.bootFetches).toBeGreaterThan(before), { timeout: 2_000 })
    await new Promise((r) => setTimeout(r, 200))
    // Before 0.46.10 this single miss went to the arbiter, which saw the
    // host ready and auto-reloaded the page (every socket dropped).
    expect(sessionStorage.getItem(RELOAD_FLAG)).toBe(null)
    expect(h.arbiterCalls).toBe(0)
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
    // The miss, then the immediate retry on a fresh socket.
    expect(h.bootFetches).toBe(before + 2)
  })
})
