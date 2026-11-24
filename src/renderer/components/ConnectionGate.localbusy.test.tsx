// @vitest-environment jsdom
// 0.46.18 — the LOCAL window, through the REAL ConnectionGate.
//
// Field report (0.46.17, mac-a under heavy compute load): one CLI request
// took 12.6 s, the next 0.1 s. The window read "offline", couldn't be used
// and took ~20 s to come back, while other saved servers were failing too
// (401s, a server that no longer answers). Rosson's rule: "If servers aren't
// responding it shouldn't impact my local daemon."
//
//   1. Another server down leaves the local window mounted and usable.
//   2. A slow local /boot-status shows the light note over the live app,
//      never the blocking overlay, and the note clears when it answers.
//   3. A slow local first connect (a reload) says the server is busy, gives
//      each probe longer, and mounts once it answers.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => {
  // Stores evaluated on import fetch at once; nothing here reaches a network.
  globalThis.fetch = (async () => new Response('{}', { status: 200 })) as typeof fetch
  return {
    app: { mounts: 0, unmounts: 0 },
    /** 'ok' answers; 'hang' never answers until the request's own timeout. */
    localBoot: 'ok' as 'ok' | 'hang',
    bootTimeouts: [] as number[],
    remoteFetches: 0,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'available', port: 4100, token: 'own' }
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
  const { RoomFailureGate } = await import('@/components/Home/room/RoomFailure')
  function App(): React.JSX.Element {
    React.useEffect(() => {
      h.app.mounts += 1
      return () => {
        h.app.unmounts += 1
      }
    }, [])
    return (
      <div data-stub-app="">
        <button type="button" data-local-input="">local</button>
        {/* A Home room pinned to server B, in the same window. */}
        <RoomFailureGate hostKey="b.k2.dev">
          <div data-b-room="" />
        </RoomFailureGate>
      </div>
    )
  }
  return { default: App, HomeRoomsHost: () => null, ZenHost: () => null, CallRingHost: () => null }
})

import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { ConnectionGate } from './ConnectionGate'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope, scopeForHost } from '@/kessel/server-scope'
import { invalidateDaemonWs } from '@/kessel/daemon-ws'
import {
  localHealthLoopsStartedForTests,
  localHealthStore,
  resetLocalHealthForTests,
} from '@/lib/local-health'
import { gridDialBackoffRemainingMs, noteGridDialFailure, resetGridDialQueueForTests } from '@/lib/grid-dial-queue'

const B_KEY = 'b.k2.dev'
const B: ConnectHost = {
  id: 'id-b', label: 'Box B', hostname: 'b.k2.dev', port: 443, secure: true,
  token: 'tok-b', remember: false, lastConnectedAt: null,
} as ConnectHost

const READY = JSON.stringify({ version: '9.9.9', protocol: 1, phase: 'ready', detail: '' })

function offlineB(): HostEntry {
  return {
    hostKey: B_KEY, saved: true, hostId: 'id-b', reach: 'offline', boot: null,
    auth: 'ok', authNote: null, role: 'member', presence: null, offlineStreak: 3, checkedAt: Date.now(),
  }
}

/** A request that never answers: it rejects when its own timeout fires,
 *  the way WebKit does for `AbortSignal.timeout`. */
function hangUntilAborted(signal: AbortSignal | null | undefined): Promise<Response> {
  return new Promise((_, reject) => {
    signal?.addEventListener('abort', () => {
      reject(Object.assign(new Error('The operation timed out.'), { name: 'TimeoutError' }))
    })
  })
}

beforeEach(() => {
  h.app.mounts = h.app.unmounts = 0
  h.localBoot = 'ok'
  h.bootTimeouts.length = 0
  h.remoteFetches = 0
  resetLocalHealthForTests()
  resetGridDialQueueForTests()
  invalidateDaemonWs()
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    if (url.includes(B_KEY)) {
      // Server B is gone: every request fails at the connection level.
      h.remoteFetches += 1
      throw new TypeError('Load failed')
    }
    if (url.includes('/boot-status')) {
      if (h.localBoot === 'hang') return hangUntilAborted(init?.signal)
      return new Response(READY, { status: 200 })
    }
    if (url.includes('/cli/local-broken')) throw new TypeError('Load failed')
    return new Response('{}', { status: 200 })
  })
  useConnectHostStore.setState({
    hosts: [B],
    activeHost: 'local',
    recovery: { kind: 'connected' },
    hydrateFromDisk: async () => undefined,
  } as never)
  hostPool.store.setState({ entries: {} })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  hostPool.store.setState({ entries: {} })
  resetLocalHealthForTests()
  resetGridDialQueueForTests()
})

function insideInert(el: Element): boolean {
  return el.closest('[inert]') !== null || el.closest('.pointer-events-none') !== null
}

async function mountedLocal(): Promise<void> {
  render(<ConnectionGate />)
  await waitFor(() => expect(h.app.mounts).toBe(1), { timeout: 5_000 })
}

describe('another server never makes the local window look offline (0.46.18)', () => {
  it('B down (pool offline, failed requests, a dial burst): only B’s room dims; the local window stays mounted and usable', async () => {
    await mountedLocal()

    // Everything server B can throw at this window at once.
    hostPool.store.setState({ entries: { [B_KEY]: offlineB() } })
    await expect(daemonCliGet(scopeForHost(B_KEY), 'presence/summary')).rejects.toThrow('Load failed')
    for (let i = 0; i < 3; i++) noteGridDialFailure(scopeForHost(B_KEY))
    expect(h.remoteFetches).toBeGreaterThan(0)

    // B's room shows its own failure, dimmed with input off…
    await waitFor(() => expect(screen.getByText('Box B is offline. Retrying…')).toBeTruthy())
    expect(insideInert(document.querySelector('[data-b-room]')!)).toBe(true)
    // …and the window's own server is untouched.
    expect(insideInert(document.querySelector('[data-local-input]')!)).toBe(false)
    expect(screen.queryByText('Connecting…')).toBeNull()
    expect(screen.queryByTestId('local-health-note')).toBeNull()
    expect(localHealthLoopsStartedForTests()).toBe(0)
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
    expect(gridDialBackoffRemainingMs(scopeForHost(B_KEY))).toBeGreaterThan(0)
    expect(gridDialBackoffRemainingMs(primaryScope())).toBe(0)
    expect(h.app.mounts).toBe(1)
    expect(h.app.unmounts).toBe(0)
  })
})

describe('a slow local server shows the light note, never the blocking overlay (0.46.18)', () => {
  it('a local request fails while /boot-status hangs: the app stays mounted, the busy note shows, then clears', async () => {
    await mountedLocal()
    h.localBoot = 'hang'

    await expect(daemonCliGet(primaryScope(), 'local-broken')).rejects.toThrow('Load failed')

    // The first probe waits out its 2 s budget, then the note shows.
    const note = await screen.findByTestId('local-health-note', undefined, { timeout: 4_000 })
    expect(note.getAttribute('data-local-health')).toBe('busy')
    expect(note.textContent).toContain('This computer’s K2 server is busy. Reconnecting…')
    // Non-blocking: it passes input through, and the app is still there.
    expect(note.style.pointerEvents).toBe('none')
    expect(screen.queryByText('Connecting…')).toBeNull()
    expect(insideInert(document.querySelector('[data-local-input]')!)).toBe(false)
    expect(h.app.mounts).toBe(1)
    expect(h.app.unmounts).toBe(0)

    // More trouble while it retries starts no second loop.
    await expect(daemonCliGet(primaryScope(), 'local-broken')).rejects.toThrow('Load failed')
    await expect(daemonCliGet(primaryScope(), 'local-broken')).rejects.toThrow('Load failed')
    expect(localHealthLoopsStartedForTests()).toBe(1)

    // The server answers again: the note goes away by itself.
    h.localBoot = 'ok'
    await waitFor(() => expect(screen.queryByTestId('local-health-note')).toBeNull(), { timeout: 4_000 })
    expect(localHealthStore.getState().health.kind).toBe('ok')
    expect(h.app.unmounts).toBe(0)
  }, 15_000)
})

describe('a slow local first connect (a reload while the server is busy)', () => {
  it('says the server is busy, not “Applying updates”, and mounts once it answers', async () => {
    h.localBoot = 'hang'
    render(<ConnectionGate />)
    await screen.findByText(
      'This computer’s K2 server is busy. Waiting for it to answer…',
      undefined,
      { timeout: 4_000 },
    )
    expect(screen.queryByText('Applying updates…')).toBeNull()
    expect(h.app.mounts).toBe(0)

    h.localBoot = 'ok'
    await waitFor(() => expect(h.app.mounts).toBe(1), { timeout: 8_000 })
    expect(screen.queryByText('Connecting…')).toBeNull()
  }, 15_000)
})
