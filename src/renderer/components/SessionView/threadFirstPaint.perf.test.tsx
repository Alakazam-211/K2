// @vitest-environment jsdom
//
// Time to first paint of a Thread (speed pass P3b), in jsdom, at the hook
// every Thread surface mounts (`useOverlayThread`).
//
// "First paint" = the first render whose `items` are not empty; measured
// from the mount, with the daemon answering after a fixed delay (a local
// read ~60 ms, a remote one over Connect ~250 ms). Cases:
//   - a Thread on this computer with a cached copy;
//   - a Thread on another server with a cached copy;
//   - a Thread with no copy (the network decides).
// The numbers printed under "[perf]" are what the P3b report quotes. The
// assertions are the budget: a cached Thread paints in the FIRST render,
// before any read is sent.

import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import type { ServerScope } from '@/kessel/server-scope'
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, renderHook, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => daemonCliGet(...a),
  daemonCliPost: async () => {
    throw new Error('no posts here')
  },
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: async () => {
    throw new Error('no socket in this test')
  },
  daemonWsBase: () => 'ws://test',
}))
vi.mock('@/stores/unread', () => ({ useAutoMarkRead: () => {} }))
vi.mock('./threadViewing', () => ({
  createViewingSender: () => ({ update() {}, dispose() {}, socketOpened() {} }),
  windowIsLookedAt: () => true,
}))

import { useOverlayThread } from './useOverlayThread'
import { resetThreadViewCacheForTests, saveThreadView } from './threadViewCache'
import { resetThreadReadsForTests } from './threadRead'

const LOCAL_MS = 60
const REMOTE_MS = 250

function scopeOf(host: string, remote: boolean): ServerScope {
  return {
    id: `host:${host}`,
    isPrimary: false,
    hostKey: host,
    connectionKey: `host:${host}`,
    isRemote: remote,
    label: host,
    connectHost: () => null,
    isWindowHost: () => false,
    serverSupports: () => true,
  } as unknown as ServerScope
}

function rows(n: number) {
  return Array.from({ length: n }, (_, i) => {
    const seq = i + 1
    return { collection: 'thread', seq, id: `t${seq}`, doc: { id: `t${seq}`, kind: 'text', from: 'k2', body: `message ${seq}` } }
  })
}

const report: string[] = []

async function measure(label: string, scope: ServerScope, cached: boolean, latencyMs: number): Promise<{ ms: number; render: number; readsBefore: number }> {
  if (cached) saveThreadView(scope.hostKey, 'k2', 'conv-1', rows(100) as never)
  daemonCliGet.mockImplementation(
    (_s: unknown, _route: string, p: Record<string, unknown>) =>
      new Promise((resolve) =>
        setTimeout(
          () =>
            resolve(
              p.after_seq !== undefined
                ? { conversation_id: 'conv-1', after_seq: p.after_seq, has_more: false, items: [], changed: [], removed: [] }
                : { conversation_id: 'conv-1', has_more: true, items: rows(100).slice(-25) },
            ),
          latencyMs,
        ),
      ),
  )
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
  let firstAt = -1
  let firstRender = -1
  let readsBefore = -1
  let n = 0
  const t0 = performance.now()
  renderHook(() => {
    const r = useOverlayThread({ scope, addr: 'k2', conversationId: 'conv-1', enabled: true })
    n++
    if (firstAt < 0 && r.items.length > 0) {
      firstAt = performance.now() - t0
      firstRender = n
      readsBefore = daemonCliGet.mock.calls.length
    }
    return r
  })
  await waitFor(() => expect(firstAt).toBeGreaterThanOrEqual(0), { timeout: 2_000 })
  warn.mockRestore()
  report.push(`[perf] ${label}: first paint ${firstAt.toFixed(1)} ms, render #${firstRender}, reads sent before it: ${readsBefore}`)
  return { ms: firstAt, render: firstRender, readsBefore }
}

beforeEach(async () => {
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  await resetThreadViewCacheForTests()
  resetThreadReadsForTests()
  daemonCliGet.mockReset()
})
afterEach(() => cleanup())
afterAll(() => {
  for (const line of report) console.log(line)
})

describe('Thread time to first paint', () => {
  it('a cached Thread on this computer paints in its first render, before any read', async () => {
    const r = await measure(`this computer, cached (read ${LOCAL_MS} ms)`, scopeOf('local', false), true, LOCAL_MS)
    expect(r.render).toBe(1)
    expect(r.readsBefore).toBe(0)
  })

  it('a cached Thread on another server paints in its first render, before any read', async () => {
    const r = await measure(`another server, cached (read ${REMOTE_MS} ms)`, scopeOf('b.example.test', true), true, REMOTE_MS)
    expect(r.render).toBe(1)
    expect(r.readsBefore).toBe(0)
  })

  it('with no copy, the read decides (the baseline)', async () => {
    const r = await measure(`this computer, no copy (read ${LOCAL_MS} ms)`, scopeOf('local', false), false, LOCAL_MS)
    expect(r.ms).toBeGreaterThanOrEqual(LOCAL_MS - 5)
    await measure(`another server, no copy (read ${REMOTE_MS} ms)`, scopeOf('b.example.test', true), false, REMOTE_MS)
  })
})
