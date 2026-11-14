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
  flushProjectsListWritesForTests,
  peekProjectsList,
  projectsListCacheKey,
  projectsListTag,
  readProjectsList,
  resetProjectsListCacheForTests,
  saveProjectsList,
  PROJECTS_LIST_CACHE_VERSION,
} from './projects-list-cache'
import { useConnectHostStore, __resetConnectHostStoreForTests, activeHostKey, type ConnectHost } from '@/stores/connect-host'

const HOST: ConnectHost = {
  id: 'h1',
  label: 'Box',
  hostname: 'box.example.test',
  username: 'ada',
  port: 443,
  secure: true,
  token: 'tok-1',
  remember: false,
  lastConnectedAt: null,
}
const HOST_KEY = activeHostKey(HOST)
const KEY = projectsListCacheKey(HOST_KEY, 'ada')

const LIST = [
  { id: 'p1', name: 'One', workspaces: [{ id: 'w1' }] },
  { id: 'p2', name: 'Two', workspaces: [] },
]

beforeEach(async () => {
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  await resetProjectsListCacheForTests()
  mem.clear()
  __resetConnectHostStoreForTests()
})

describe('projects list cache', () => {
  it('is saved on this computer and read back after a reload, with its tag', async () => {
    saveProjectsList(KEY, HOST_KEY, LIST, 'W/"abc"')
    expect(peekProjectsList(KEY)?.projects).toEqual(LIST)
    await flushProjectsListWritesForTests()
    await resetProjectsListCacheForTests()
    expect(peekProjectsList(KEY)).toBeNull()
    const back = await readProjectsList(KEY)
    expect(back?.projects).toEqual(LIST)
    expect(back?.tag).toBe('W/"abc"')
    // Read once, it is in memory again.
    expect(peekProjectsList(KEY)?.projects).toEqual(LIST)
  })

  it('keeps one list per server login', async () => {
    saveProjectsList(KEY, HOST_KEY, LIST, null)
    expect(await readProjectsList(projectsListCacheKey(HOST_KEY, 'grace'))).toBeNull()
    expect(await readProjectsList(projectsListCacheKey('local', ''))).toBeNull()
  })

  it('ignores a row of another version', async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const r = indexedDB.open('k2.projects-list-cache', 1)
      r.onupgradeneeded = () => r.result.createObjectStore('lists', { keyPath: 'key' }).createIndex('savedAt', 'savedAt')
      r.onsuccess = () => resolve(r.result)
      r.onerror = () => reject(r.error)
    })
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('lists', 'readwrite')
      tx.objectStore('lists').put({ key: KEY, hostKey: HOST_KEY, v: PROJECTS_LIST_CACHE_VERSION + 1, projects: LIST, tag: null, savedAt: 1 })
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error)
    })
    db.close()
    expect(await readProjectsList(KEY)).toBeNull()
  })

  it('forgets a server when its login is signed out, keeps it across a token refresh', async () => {
    useConnectHostStore.setState({ hosts: [HOST] })
    saveProjectsList(KEY, HOST_KEY, LIST, null)
    await flushProjectsListWritesForTests()
    // Same user, new token: kept.
    useConnectHostStore.setState({ hosts: [{ ...HOST, token: 'tok-2' }] })
    expect(peekProjectsList(KEY)).not.toBeNull()
    // Signed out: gone from memory and disk.
    useConnectHostStore.setState({ hosts: [{ ...HOST, token: '' }] })
    expect(peekProjectsList(KEY)).toBeNull()
    await flushProjectsListWritesForTests()
    await resetProjectsListCacheForTests()
    expect(await readProjectsList(KEY)).toBeNull()
  })

  it('works out the same tag the daemon sends (shared fixture with response_wire.rs)', async () => {
    // The daemon serialises this list as exactly these bytes; the renderer
    // holds the parsed list and must reach the same tag from it.
    const wire = '[{"id":"p1","name":"Café \\"one\\"","tabOrder":3,"iconUrl":null,"workspaces":[]}]'
    expect(await projectsListTag(JSON.parse(wire))).toBe('W/"a9de4e57e561bbb8bdc197b3faee8112"')
  })
})
