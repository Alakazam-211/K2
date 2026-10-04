// @vitest-environment jsdom
//
// Home avatars over the real `daemonCli*` wiring (prd-home-picker-and-remote-
// avatars-v1 T4.2, P10, P14): the app's own sync instance, with `fetch`
// stubbed. A row on server B is fetched from B's base with B's own token;
// the bytes go to THIS computer's daemon (loopback, owner token) and never
// to B; the local daemon is never asked to fetch anything remote.

import { beforeEach, describe, expect, it, vi } from 'vitest'

// Store modules read the daemon at import time; answer those quietly until
// the test's own fetch is installed.
vi.hoisted(() => {
  ;(globalThis as { fetch: unknown }).fetch = async (): Promise<Response> => new Response('[]', { status: 200 })
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) =>
    cmd === 'daemon_ws_url' ? { state: 'available', port: 4111, token: 'own-tok' } : null,
  ),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { homeAvatarSync, useHomeAvatarStore } from './home-avatars'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'

const boxB: ConnectHost = {
  id: 'b',
  label: 'Box B',
  hostname: 'b.k2.dev',
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: true,
  lastConnectedAt: null,
}

const requests: { method: string; url: string; body: string | null }[] = []
const disk = new Map<string, { dataUrl: string | null; missing: boolean; fetchedAt: number; sha256: string | null }>()

function json(v: unknown, status = 200): Response {
  return new Response(JSON.stringify(v), { status, headers: { 'Content-Type': 'application/json' } })
}

const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
  const url = String(input instanceof Request ? input.url : input)
  const method = init?.method ?? 'GET'
  const body = typeof init?.body === 'string' ? init.body : null
  requests.push({ method, url, body })
  const u = new URL(url)
  if (u.origin === 'http://127.0.0.1:4111') {
    if (u.searchParams.get('token') !== 'own-tok') return json({ error: 'bad token' }, 403)
    if (u.pathname === '/cli/home/avatars' && method === 'GET') {
      const out: Record<string, unknown> = {}
      for (const a of (u.searchParams.get('addresses') ?? '').split(',')) {
        const e = disk.get(a)
        if (e) out[a] = e
      }
      return json({ ok: true, avatars: out })
    }
    if (u.pathname === '/cli/home/avatars/put' && method === 'POST') {
      const b = JSON.parse(body ?? '{}') as { address: string; dataUrl: string | null }
      disk.set(b.address, { dataUrl: b.dataUrl, missing: b.dataUrl === null, fetchedAt: Date.now(), sha256: null })
      return json({ ok: true, changed: true })
    }
  }
  if (u.origin === 'https://b.k2.dev') {
    if (u.searchParams.get('token') !== 'tok-b') return json({ error: 'bad token' }, 401)
    if (u.pathname === '/cli/projects/list') {
      return json([{ id: 'pb', name: 'Bee', handle: 'bee', path: '/srv/bee', iconUrl: null }])
    }
    if (u.pathname === '/cli/projects/get-icon') return json({ found: false, dataUrl: null })
  }
  throw new Error(`unexpected ${method} ${url}`)
})

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  requests.length = 0
  disk.clear()
  useHomeAvatarStore.setState({ cached: {}, local: {} })
  __resetConnectHostStoreForTests()
  useConnectHostStore.setState({ hosts: [boxB], connectionStatus: 'connected' })
})

describe('the app’s avatar sync over daemonCli*', () => {
  it('T4.2: B is asked with B’s token at B’s base; found:false is put as null on this computer’s daemon', async () => {
    const row = { address: 'bee::b.k2.dev', workspaceId: 'pb', label: 'Bee' }
    await homeAvatarSync.refresh({
      rows: [row],
      connectedKey: 'local',
      connectedProjects: [],
      reachable: (k) => k === 'b.k2.dev',
    })
    const seen = requests.map((r) => {
      const u = new URL(r.url)
      return `${r.method} ${u.origin}${u.pathname}`
    })
    expect(seen).toEqual([
      'GET http://127.0.0.1:4111/cli/home/avatars',
      'GET https://b.k2.dev/cli/projects/list',
      'GET https://b.k2.dev/cli/projects/get-icon',
      'POST http://127.0.0.1:4111/cli/home/avatars/put',
      'GET http://127.0.0.1:4111/cli/home/avatars',
    ])
    const icon = new URL(requests[2].url)
    expect(icon.searchParams.get('token')).toBe('tok-b')
    expect(icon.searchParams.get('path')).toBe('/srv/bee')
    expect(icon.searchParams.get('project_id')).toBe('pb')
    expect(new URL(requests[1].url).searchParams.get('token')).toBe('tok-b')
    expect(JSON.parse(requests[3].body ?? 'null')).toEqual({ address: row.address, dataUrl: null })
    // Nothing from this computer's daemon went to B, and B's token never went to this computer.
    for (const r of requests) {
      const u = new URL(r.url)
      if (u.origin === 'https://b.k2.dev') expect(u.searchParams.get('token')).toBe('tok-b')
      else expect(r.url.includes('tok-b') || (r.body ?? '').includes('tok-b')).toBe(false)
    }
    expect(useHomeAvatarStore.getState().cached[row.address]?.missing).toBe(true)
  })
})
