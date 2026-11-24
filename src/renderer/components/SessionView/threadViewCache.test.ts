import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import {
  flushThreadViewWritesForTests,
  peekThreadView,
  readThreadView,
  resetThreadViewCacheForTests,
  saveThreadView,
  evictionPlan,
  listThreadViews,
  MEMORY_MAX_CHATS,
  THREAD_VIEW_CACHE_ITEMS,
  THREAD_VIEW_CACHE_MAX_BYTES,
  THREAD_VIEW_CACHE_THREADS,
  THREAD_VIEW_CACHE_VERSION,
} from './threadViewCache'
import type { OverlayThreadItem } from './overlayThread'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'

function items(n: number, from = 1): OverlayThreadItem[] {
  return Array.from({ length: n }, (_, i) => {
    const seq = from + i
    return { collection: 'thread', seq, id: `t${seq}`, doc: { id: `t${seq}`, kind: 'text', from: 'k2', body: `m${seq}` } }
  })
}

const HOST: ConnectHost = {
  id: 'h1',
  label: 'Rosson',
  hostname: 'rosson.k2.dev',
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'tok-1',
  remember: false,
  lastConnectedAt: null,
}

beforeEach(async () => {
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  await resetThreadViewCacheForTests({ keepDisk: true })
  mem.clear()
  __resetConnectHostStoreForTests()
})

describe('thread view cache', () => {
  it('keeps the newest 100 Thread messages per Thread, never chatter, never a local send time', () => {
    const mixed: OverlayThreadItem[] = [
      ...items(140),
      { collection: 'thread', seq: 141, id: 'c141', doc: { id: 'c141', kind: 'chatter', from: 'x', body: 'a2a' } },
      { collection: 'thread', seq: 142, id: 't142', localTime: true, doc: { id: 't142', kind: 'text', from: 'me', via: 'compose', body: 'mine', created_at: 5 } },
    ]
    saveThreadView('rosson.k2.dev', 'k2', 'conv-1', mixed)
    const hit = peekThreadView('rosson.k2.dev', 'k2')!
    expect(THREAD_VIEW_CACHE_ITEMS).toBe(100)
    expect(hit.items).toHaveLength(THREAD_VIEW_CACHE_ITEMS)
    expect(hit.items[0].seq).toBe(42)
    // Cut to the newest 100: older messages exist.
    expect(hit.hasMore).toBe(true)
    expect(hit.items.at(-1)).toEqual({ collection: 'thread', seq: 142, id: 't142', doc: expect.objectContaining({ body: 'mine' }) })
    expect(hit.items.at(-1)).not.toHaveProperty('localTime')
    expect(hit.items.some((it) => it.id === 'c141')).toBe(false)
    expect(hit.conversationId).toBe('conv-1')
    // Per (server, address): another server's Thread with the same address is another entry.
    expect(peekThreadView('zoe.k2.dev', 'k2')).toBeNull()
    expect(peekThreadView('rosson.k2.dev', 'other')).toBeNull()
  })

  it('is saved on this computer (IndexedDB) and read back after a reload, versioned', async () => {
    saveThreadView('rosson.k2.dev', 'k2', 'conv-1', items(3))
    await flushThreadViewWritesForTests()
    await resetThreadViewCacheForTests({ keepDisk: true })
    expect(peekThreadView('rosson.k2.dev', 'k2')).toBeNull()
    const back = await readThreadView('rosson.k2.dev', 'k2')
    expect(back?.items.map((it) => it.id)).toEqual(['t1', 't2', 't3'])
    // Read once, it is in memory again.
    expect(peekThreadView('rosson.k2.dev', 'k2')?.items).toHaveLength(3)
    // A row of another version is ignored.
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const r = indexedDB.open('k2.thread-view-cache', 3)
      r.onsuccess = () => resolve(r.result)
      r.onerror = () => reject(r.error)
    })
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('threads', 'readwrite')
      tx.objectStore('threads').put({
        key: 'rosson.k2.dev|v2',
        v: THREAD_VIEW_CACHE_VERSION + 1,
        hostKey: 'rosson.k2.dev',
        addr: 'v2',
        conversationId: '',
        items: items(1),
        savedAt: 1,
      })
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error)
    })
    db.close()
    expect(await readThreadView('rosson.k2.dev', 'v2')).toBeNull()
  })

  it('is bounded: 40 Threads in memory, 150 on this computer, least recently saved out first', async () => {
    expect([MEMORY_MAX_CHATS, THREAD_VIEW_CACHE_THREADS]).toEqual([40, 150])
    for (let i = 0; i < THREAD_VIEW_CACHE_THREADS; i++) {
      saveThreadView('rosson.k2.dev', `a${i}`, '', items(1), 1_000 + i)
    }
    await flushThreadViewWritesForTests()
    // Memory: the newest 40 saves; the rest were written before they left.
    expect(peekThreadView('rosson.k2.dev', `a${THREAD_VIEW_CACHE_THREADS - 1}`)).not.toBeNull()
    expect(peekThreadView('rosson.k2.dev', `a${THREAD_VIEW_CACHE_THREADS - MEMORY_MAX_CHATS}`)).not.toBeNull()
    expect(peekThreadView('rosson.k2.dev', `a${THREAD_VIEW_CACHE_THREADS - MEMORY_MAX_CHATS - 1}`)).toBeNull()
    expect(await listThreadViews('rosson.k2.dev')).toHaveLength(THREAD_VIEW_CACHE_THREADS)
    // a0 saved again is the newest; one more Thread pushes the oldest (a1) out.
    saveThreadView('rosson.k2.dev', 'a0', '', items(2), 9_000)
    await flushThreadViewWritesForTests()
    saveThreadView('rosson.k2.dev', 'new', '', items(1), 9_001)
    await flushThreadViewWritesForTests()
    await resetThreadViewCacheForTests({ keepDisk: true })
    expect(await listThreadViews('rosson.k2.dev')).toHaveLength(THREAD_VIEW_CACHE_THREADS)
    expect(await readThreadView('rosson.k2.dev', 'a1')).toBeNull()
    expect((await readThreadView('rosson.k2.dev', 'a0'))?.items).toHaveLength(2)
    expect(await readThreadView('rosson.k2.dev', 'a2')).not.toBeNull()
    expect(await readThreadView('rosson.k2.dev', 'new')).not.toBeNull()
  })

  it('is bounded by bytes too (15 MB): the oldest go until it fits, never the one just saved', () => {
    expect(THREAD_VIEW_CACHE_MAX_BYTES).toBe(15 * 1024 * 1024)
    const mb = 1024 * 1024
    const metas = [
      { key: 'old', savedAt: 1, bytes: 6 * mb },
      { key: 'mid', savedAt: 2, bytes: 6 * mb },
      { key: 'new', savedAt: 3, bytes: 6 * mb },
    ]
    expect(evictionPlan(metas, 'new')).toEqual(['old'])
    // The one just saved stays even when it is the oldest by its clock.
    expect(evictionPlan(metas, 'old')).toEqual(['mid'])
    expect(evictionPlan(metas.slice(0, 2), 'mid')).toEqual([])
    // Count and bytes together.
    expect(evictionPlan(metas, 'new', 1, 100 * mb)).toEqual(['old', 'mid'])
  })

  it('a saved Thread lists its cursor: the newest seq a server read brought, never a pushed one', async () => {
    const pushed: OverlayThreadItem = { ...items(1, 9)[0], provisional: true }
    saveThreadView('rosson.k2.dev', 'k2', 'conv-1', [...items(5), pushed])
    await flushThreadViewWritesForTests()
    await resetThreadViewCacheForTests({ keepDisk: true })
    const [meta] = await listThreadViews('rosson.k2.dev')
    expect(meta).toMatchObject({ addr: 'k2', conversationId: 'conv-1', cursor: 5 })
    expect((await readThreadView('rosson.k2.dev', 'k2'))?.items.at(-1)?.provisional).toBe(true)
  })

  it('is cleared for a server on sign-out and when the server is removed', async () => {
    useConnectHostStore.getState().addHost(HOST)
    saveThreadView('rosson.k2.dev', 'k2', '', items(2))
    saveThreadView('zoe.k2.dev', 'k2', '', items(2))
    await flushThreadViewWritesForTests()
    // Sign-out: the saved login's token goes.
    useConnectHostStore.setState((s) => ({ hosts: s.hosts.map((h) => ({ ...h, token: '' })) }))
    await vi.waitFor(async () => expect(await readThreadView('rosson.k2.dev', 'k2')).toBeNull())
    expect(peekThreadView('zoe.k2.dev', 'k2')).not.toBeNull()
    // Removal.
    useConnectHostStore.setState((s) => ({ hosts: s.hosts.map((h) => ({ ...h, token: 'tok-2' })) }))
    saveThreadView('rosson.k2.dev', 'k2', '', items(2))
    await flushThreadViewWritesForTests()
    useConnectHostStore.getState().removeHost('h1')
    await vi.waitFor(async () => expect(await readThreadView('rosson.k2.dev', 'k2')).toBeNull())
  })

  it('works with no IndexedDB at all (memory only, nothing thrown)', async () => {
    ;(globalThis as { indexedDB?: IDBFactory }).indexedDB = undefined
    await resetThreadViewCacheForTests({ keepDisk: true })
    saveThreadView('rosson.k2.dev', 'k2', '', items(2))
    await flushThreadViewWritesForTests()
    expect((await readThreadView('rosson.k2.dev', 'k2'))?.items).toHaveLength(2)
    expect(await readThreadView('rosson.k2.dev', 'none')).toBeNull()
  })

  it('keeps a deleted message deleted across a reload, and drops copies saved before the flag was kept', async () => {
    const tomb: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 'd4',
      doc: { id: 'd4', kind: 'text', from: 'owner', via: 'compose', body: null, author: { kind: 'owner', login: 'owner' }, deleted: true, deletedAt: 9, deletedBy: { kind: 'owner', login: 'owner' } },
    }
    saveThreadView('rosson.k2.dev', 'k2', 'conv-1', [...items(3), tomb])
    await flushThreadViewWritesForTests()
    await resetThreadViewCacheForTests({ keepDisk: true })
    const back = await readThreadView('rosson.k2.dev', 'k2')
    const doc = back?.items.find((it) => it.id === 'd4')?.doc
    expect(doc?.deleted).toBe(true)
    expect(doc?.body).toBeNull()
    // Version 1 copies could hold a deleted message without its flag (an
    // empty bubble until the read landed): they are not painted.
    expect(THREAD_VIEW_CACHE_VERSION).toBeGreaterThanOrEqual(2)
  })
})
