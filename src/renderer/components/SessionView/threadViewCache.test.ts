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
  THREAD_VIEW_CACHE_ITEMS,
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
  it('keeps the newest 25 Thread messages per Thread, never chatter, never a local send time', () => {
    const mixed: OverlayThreadItem[] = [
      ...items(40),
      { collection: 'thread', seq: 41, id: 'c41', doc: { id: 'c41', kind: 'chatter', from: 'x', body: 'a2a' } },
      { collection: 'thread', seq: 42, id: 't42', localTime: true, doc: { id: 't42', kind: 'text', from: 'me', via: 'compose', body: 'mine', created_at: 5 } },
    ]
    saveThreadView('rosson.k2.dev', 'k2', 'conv-1', mixed)
    const hit = peekThreadView('rosson.k2.dev', 'k2')!
    expect(hit.items).toHaveLength(THREAD_VIEW_CACHE_ITEMS)
    expect(hit.items[0].seq).toBe(17)
    expect(hit.items.at(-1)).toEqual({ collection: 'thread', seq: 42, id: 't42', doc: expect.objectContaining({ body: 'mine' }) })
    expect(hit.items.at(-1)).not.toHaveProperty('localTime')
    expect(hit.items.some((it) => it.id === 'c41')).toBe(false)
    expect(hit.conversationId).toBe('conv-1')
    // Per (server, address): another server's Thread with the same address is another entry.
    expect(peekThreadView('z3thon.k2.dev', 'k2')).toBeNull()
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
      const r = indexedDB.open('k2.thread-view-cache', 1)
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

  it('is bounded: 50 Threads, least recently used out first (memory and disk)', async () => {
    for (let i = 0; i < THREAD_VIEW_CACHE_THREADS; i++) {
      saveThreadView('rosson.k2.dev', `a${i}`, '', items(1), 1_000 + i)
      await flushThreadViewWritesForTests()
    }
    // Touch the oldest: a0 is now the most recent.
    await readThreadView('rosson.k2.dev', 'a0')
    saveThreadView('rosson.k2.dev', 'a0', '', items(2), 9_000)
    await flushThreadViewWritesForTests()
    saveThreadView('rosson.k2.dev', 'new', '', items(1), 9_001)
    await flushThreadViewWritesForTests()
    expect(peekThreadView('rosson.k2.dev', 'a0')).not.toBeNull()
    expect(peekThreadView('rosson.k2.dev', 'a1')).toBeNull()
    expect(peekThreadView('rosson.k2.dev', 'new')).not.toBeNull()
    await resetThreadViewCacheForTests({ keepDisk: true })
    expect(await readThreadView('rosson.k2.dev', 'a1')).toBeNull()
    expect((await readThreadView('rosson.k2.dev', 'a0'))?.items).toHaveLength(2)
    expect(await readThreadView('rosson.k2.dev', 'a2')).not.toBeNull()
  })

  it('is cleared for a server on sign-out and when the server is removed', async () => {
    useConnectHostStore.getState().addHost(HOST)
    saveThreadView('rosson.k2.dev', 'k2', '', items(2))
    saveThreadView('z3thon.k2.dev', 'k2', '', items(2))
    await flushThreadViewWritesForTests()
    // Sign-out: the saved login's token goes.
    useConnectHostStore.setState((s) => ({ hosts: s.hosts.map((h) => ({ ...h, token: '' })) }))
    await vi.waitFor(async () => expect(await readThreadView('rosson.k2.dev', 'k2')).toBeNull())
    expect(peekThreadView('z3thon.k2.dev', 'k2')).not.toBeNull()
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
})
