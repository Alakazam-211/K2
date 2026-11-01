// @vitest-environment jsdom
// research-remote-thread-load-v1 Fix 4: a remote Thread paints its view
// cache at once, then the server's read replaces it; the cache never marks
// anything read. Fix 1: the first page read is shared with a prefetch.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import type { ServerScope } from '@/kessel/server-scope'
import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest'
import { cleanup, renderHook, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const markReadOnServer = vi.hoisted(() => vi.fn(async (..._a: unknown[]) => {}))

// Only Thread routes reach the spies (other modules call the daemon on
// import; those get a loud rejection their stores handle).
vi.mock('@/lib/daemon-cli', () => {
  const routed =
    (spy: (...a: unknown[]) => unknown) =>
    (scope: unknown, route: string, ...rest: unknown[]) =>
      route === 'thread' || route.startsWith('thread/')
        ? spy(scope, route, ...rest)
        : Promise.reject(new Error(`useOverlayThread.cache.test: route not mocked: ${route}`))
  return { daemonCliGet: routed(daemonCliGet), daemonCliPost: routed(daemonCliPost) }
})
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: async () => {
    throw new Error('no socket in this test')
  },
  daemonWsBase: () => 'ws://test',
}))
vi.mock('@/stores/unread', () => ({
  useAutoMarkRead: (opts: { visible: boolean; newestSeq: number | null; id: string | null; scope: ServerScope }) => {
    if (opts.visible && opts.id && typeof opts.newestSeq === 'number' && opts.newestSeq > 0) void markReadOnServer(opts.scope, 'thread', opts.id, opts.newestSeq)
  },
}))
vi.mock('./threadViewing', () => ({
  createViewingSender: () => ({ update() {}, dispose() {}, socketOpened() {} }),
  windowIsLookedAt: () => true,
}))

import { useOverlayThread } from './useOverlayThread'
import { peekThreadView, resetThreadViewCacheForTests, saveThreadView } from './threadViewCache'
import { prefetchThread, resetThreadReadsForTests } from './threadRead'

function remoteScope(host = 'rosson.k2.dev'): ServerScope {
  return {
    id: `host:${host}`,
    isPrimary: false,
    hostKey: host,
    connectionKey: `host:${host}`,
    isRemote: true,
    label: host,
    connectHost: () => null,
    isWindowHost: () => false,
    creds: async () => {
      throw new Error('no creds')
    },
    httpBase: async () => `https://${host}`,
    wsBase: async () => `wss://${host}`,
    serverSupports: () => false,
  } as unknown as ServerScope
}

function page(from: number, to: number, body = 'm'): { conversation_id: string; has_more: boolean; items: unknown[] } {
  const items = []
  for (let seq = from; seq <= to; seq++) {
    items.push({ collection: 'thread', seq, id: `t${seq}`, doc: { id: `t${seq}`, kind: 'text', from: 'k2', body: `${body}${seq}` } })
  }
  // conversation '' keeps the socket closed in this test.
  return { conversation_id: '', has_more: false, items }
}

beforeEach(async () => {
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  await resetThreadViewCacheForTests()
  resetThreadReadsForTests()
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  markReadOnServer.mockClear()
})

afterEach(() => cleanup())

describe('remote Thread view cache in the hook', () => {
  it('paints the cached messages at once, then the server read replaces them; nothing is marked read from the cache', async () => {
    const scope = remoteScope()
    saveThreadView('rosson.k2.dev', 'k2', 'conv-1', page(1, 3, 'old').items as never)
    let answer!: (v: unknown) => void
    daemonCliGet.mockReturnValueOnce(new Promise((r) => (answer = r)))
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { result } = renderHook(() => useOverlayThread({ scope, addr: 'k2', conversationId: 'conv-1', enabled: true, viewing: true }))
    // First render after the effect: the cache, before any answer.
    await waitFor(() => expect(result.current.items.map((it) => it.doc.body)).toEqual(['old1', 'old2', 'old3']))
    expect(result.current.fromCache).toBe(true)
    expect(result.current.loaded).toBe(true)
    expect(markReadOnServer).not.toHaveBeenCalled()
    answer({ ...page(2, 4, 'new'), conversation_id: '' })
    await waitFor(() => expect(result.current.items.map((it) => it.doc.body)).toEqual(['new2', 'new3', 'new4']))
    expect(result.current.fromCache).toBe(false)
    // The server's read is what gets saved, and only it marks read.
    await waitFor(() => expect(peekThreadView('rosson.k2.dev', 'k2')?.items.map((it) => it.id)).toEqual(['t2', 't3', 't4']))
    await waitFor(() => expect(markReadOnServer).toHaveBeenCalled())
    expect(markReadOnServer.mock.calls.every((c) => (c as unknown[])[3] === 4)).toBe(true)
    warn.mockRestore()
  })

  it('a failed read keeps the cached view up, still not marked read', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    saveThreadView('rosson.k2.dev', 'k2', '', page(1, 2, 'old').items as never)
    daemonCliGet.mockRejectedValueOnce(new Error('tunnel down'))
    const scope = remoteScope()
    const { result } = renderHook(() =>
      useOverlayThread({ scope, addr: 'k2', conversationId: 'conv-1', enabled: true, viewing: true }),
    )
    await waitFor(() => expect(result.current.error).toBe('tunnel down'))
    expect(result.current.items.map((it) => it.doc.body)).toEqual(['old1', 'old2'])
    expect(result.current.fromCache).toBe(true)
    expect(markReadOnServer).not.toHaveBeenCalled()
    warn.mockRestore()
  })

  it('without a cached copy it waits for the server as before', async () => {
    let answer!: (v: unknown) => void
    daemonCliGet.mockReturnValueOnce(new Promise((r) => (answer = r)))
    const scope = remoteScope()
    const { result } = renderHook(() => useOverlayThread({ scope, addr: 'k2', conversationId: null, enabled: true }))
    await new Promise((r) => setTimeout(r, 10))
    expect(result.current.loaded).toBe(false)
    expect(result.current.items).toEqual([])
    answer(page(1, 1))
    await waitFor(() => expect(result.current.items).toHaveLength(1))
  })

  it('a view opening during a prefetch of the same Thread waits for it: one GET', async () => {
    let answer!: (v: unknown) => void
    daemonCliGet.mockReturnValueOnce(new Promise((r) => (answer = r)))
    const scope = remoteScope()
    const pre = prefetchThread(scope, 'k2')
    const { result } = renderHook(() => useOverlayThread({ scope, addr: 'k2', conversationId: null, enabled: true }))
    answer(page(1, 2))
    await waitFor(() => expect(result.current.items).toHaveLength(2))
    expect(await pre).toBe(true)
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
    expect(daemonCliGet).toHaveBeenCalledWith(scope, 'thread', { addr: 'k2', limit: 25 })
  })
})
