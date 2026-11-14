// Home groups: the person's sends stamp the agent's rows, rate-limited to
// one stamp per row per 30 s (with one trailing stamp so the last send
// still counts), and only this window's own accepted Thread posts and card
// answers count. An agent's message to the person never passes through a
// POST from this window, so it never stamps.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { STAMP_MIN_GAP_MS, createSendStamper, rowAddressForSend } from './home-row-stamp'
import { USER_SEND_ROUTES, noteUserPost, onUserSend } from './user-send-bus'
import { scopeForHost, __resetServerScopesForTests } from '@/kessel/server-scope'
import { invalidateDaemonWs } from '@/kessel/daemon-ws'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { __resetRemoteSessionForTests } from '@/lib/remote-session'

describe('rowAddressForSend', () => {
  it('names the agent: a sidecar address is its agent, case folded', () => {
    expect(rowAddressForSend('rosson.k2.dev', 'Nova')).toBe('nova::rosson.k2.dev')
    expect(rowAddressForSend('local', 'k2/3')).toBe('k2::local')
    expect(rowAddressForSend('local', '  ')).toBeNull()
  })
})

describe('createSendStamper', () => {
  it('stamps at once, then at most once per row per 30 s, with one trailing stamp', () => {
    let now = 1_000_000
    const stamps: Array<[string, number]> = []
    const timers: Array<{ fn: () => void; at: number }> = []
    const send = createSendStamper({
      stamp: (a, at) => stamps.push([a, at]),
      now: () => now,
      schedule: (fn, ms) => timers.push({ fn, at: now + ms }),
    })
    send('local', 'nova')
    expect(stamps).toEqual([['nova::local', 1_000_000]])
    // A burst inside the window: no stamp now, one timer for the trailing stamp.
    for (const dt of [1000, 2000, 5000]) {
      now = 1_000_000 + dt
      send('local', 'nova')
    }
    expect(stamps.length).toBe(1)
    expect(timers.map((t) => t.at)).toEqual([1_000_000 + STAMP_MIN_GAP_MS])
    // Another agent is its own row: stamped at once.
    send('local', 'bee')
    expect(stamps.at(-1)).toEqual(['bee::local', 1_005_000])
    // The window ends: the latest send in it is stamped once.
    now = 1_000_000 + STAMP_MIN_GAP_MS
    timers[0].fn()
    expect(stamps.filter(([a]) => a === 'nova::local')).toEqual([
      ['nova::local', 1_000_000],
      ['nova::local', 1_005_000],
    ])
    // After a quiet window, a send stamps at once again.
    now += STAMP_MIN_GAP_MS
    send('local', 'nova')
    expect(stamps.at(-1)).toEqual(['nova::local', now])
  })
})

const A: ConnectHost = {
  id: 'a',
  label: 'A',
  hostname: 'rosson.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-a',
  remember: false,
  lastConnectedAt: null,
}

function res(status: number, body: unknown): Response {
  return { ok: status < 400, status, url: '', text: async () => JSON.stringify(body) } as unknown as Response
}

describe('which sends count', () => {
  let seen: Array<[string, string]> = []
  let fetchMock: ReturnType<typeof vi.fn>
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    __resetServerScopesForTests()
    invalidateDaemonWs()
    __resetRemoteSessionForTests()
    invokeMock.mockReset()
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'daemon_ws_url') return { state: 'available', port: 50123, token: 'local-tok' }
      return null
    })
    useConnectHostStore.getState().addHost(A)
    fetchMock = vi.fn(async () => res(200, { ok: true }))
    vi.stubGlobal('fetch', fetchMock)
    seen = []
  })

  it('a Thread post and a card answer the server took count; on that server', async () => {
    const off = onUserSend((host, addr) => seen.push([host, addr]))
    try {
      await daemonCliPost(scopeForHost(A), 'thread/post', { addr: 'nova', text: 'hi', via: 'compose' })
      await daemonCliPost(scopeForHost(A), 'thread/answer', { addr: 'nova', id: 'c1', answer: 'yes' })
      expect(seen).toEqual([
        ['rosson.k2.dev', 'nova'],
        ['rosson.k2.dev', 'nova'],
      ])
      expect([...USER_SEND_ROUTES].sort()).toEqual(['thread/answer', 'thread/post'])
    } finally {
      off()
    }
  })

  it('an agent’s message (read with GET), a refused post, and other writes never count', async () => {
    const off = onUserSend((host, addr) => seen.push([host, addr]))
    try {
      // The agent's reply reaches this window by reading the Thread.
      fetchMock.mockImplementation(async () => res(200, { items: [{ from: 'nova', body: 'done' }] }))
      await daemonCliGet(scopeForHost(A), 'thread', { addr: 'nova' })
      fetchMock.mockImplementation(async () => res(200, { ok: true }))
      await daemonCliPost(scopeForHost(A), 'thread/read', { addr: 'nova' })
      fetchMock.mockImplementation(async () => res(409, { error: 'refused' }))
      await expect(daemonCliPost(scopeForHost(A), 'thread/post', { addr: 'nova', text: 'x' })).rejects.toThrow()
      expect(seen).toEqual([])
    } finally {
      off()
    }
  })

  it('a listener that throws never breaks the send', () => {
    const off = onUserSend(() => {
      throw new Error('boom')
    })
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    try {
      expect(() => noteUserPost('thread/post', 'local', { addr: 'nova' })).not.toThrow()
      expect(errors).toHaveBeenCalledTimes(1)
    } finally {
      off()
      errors.mockRestore()
    }
  })
})
