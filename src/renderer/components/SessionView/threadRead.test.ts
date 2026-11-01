import { describe, it, expect, vi, beforeEach } from 'vitest'

const getMock = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => getMock(...a),
  daemonCliPost: async () => {
    throw new Error('no posts here')
  },
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import type { ServerScope } from '@/kessel/server-scope'
import { prefetchThread, PREFETCH_FRESH_MS, readThreadFirstPage, resetThreadReadsForTests } from './threadRead'
import { peekThreadView, resetThreadViewCacheForTests } from './threadViewCache'

function scope(remote: boolean, host = remote ? 'rosson.k2.dev' : 'local'): ServerScope {
  return { hostKey: host, connectionKey: `host:${host}`, isRemote: remote, serverSupports: () => false } as unknown as ServerScope
}

const page = { conversation_id: 'c1', has_more: false, items: [{ collection: 'thread', seq: 1, id: 't1', doc: { id: 't1', kind: 'text', from: 'k2', body: 'hi' } }] }

beforeEach(async () => {
  getMock.mockReset()
  resetThreadReadsForTests()
  await resetThreadViewCacheForTests()
})

describe('the shared first page read', () => {
  it('a remote read is shared while on its way and fills the view cache', async () => {
    let answer!: (v: unknown) => void
    getMock.mockReturnValueOnce(new Promise((r) => (answer = r)))
    const s = scope(true)
    const a = readThreadFirstPage(s, 'k2')
    const b = readThreadFirstPage(s, 'k2')
    answer(page)
    expect(await a).toBe(page)
    expect(await b).toBe(page)
    expect(getMock).toHaveBeenCalledTimes(1)
    expect(peekThreadView('rosson.k2.dev', 'k2')?.items.map((it) => it.id)).toEqual(['t1'])
    // Done: the next read goes out again.
    getMock.mockResolvedValueOnce(page)
    await readThreadFirstPage(s, 'k2')
    expect(getMock).toHaveBeenCalledTimes(2)
  })

  it('a local read is never shared or cached', async () => {
    getMock.mockResolvedValue(page)
    const s = scope(false)
    await Promise.all([readThreadFirstPage(s, 'k2'), readThreadFirstPage(s, 'k2')])
    expect(getMock).toHaveBeenCalledTimes(2)
    expect(peekThreadView('local', 'k2')).toBeNull()
  })
})

describe('prefetchThread', () => {
  it('at most one in flight; a Thread cached in the last 30 s is skipped; never throws', async () => {
    let answer!: (v: unknown) => void
    getMock.mockReturnValueOnce(new Promise((r) => (answer = r)))
    const s = scope(true)
    const first = prefetchThread(s, 'a')
    expect(await prefetchThread(s, 'b')).toBe(false)
    answer(page)
    expect(await first).toBe(true)
    expect(getMock).toHaveBeenCalledTimes(1)
    // Fresh: skipped.
    expect(await prefetchThread(s, 'a')).toBe(false)
    // Older than 30 s: read again; a failure is swallowed.
    const debug = vi.spyOn(console, 'debug').mockImplementation(() => {})
    getMock.mockRejectedValueOnce(new Error('offline'))
    expect(await prefetchThread(s, 'a', () => Date.now() + PREFETCH_FRESH_MS + 1)).toBe(true)
    expect(getMock).toHaveBeenCalledTimes(2)
    debug.mockRestore()
    // Local: nothing.
    expect(await prefetchThread(scope(false), 'a')).toBe(false)
    expect(getMock).toHaveBeenCalledTimes(2)
  })
})
