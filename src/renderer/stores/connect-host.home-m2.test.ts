// Home M2 — the saved-server list across edits and windows.
//
//   - MS61: an edit that changes a server's address re-keys Home rows,
//     host-prefixed storage and pool entries, and refreshes `activeHost`
//     when it is the window's server. A second entry for a server that is
//     already saved is refused ("Already saved as …"); entries that were
//     duplicates before this rule keep working.
//   - MS62: another window's write of the list merges in through the
//     `storage` event, keeping this window's in-memory tokens and picking
//     up a newly saved server's remembered token.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const h = vi.hoisted(() => {
  const mem = new Map<string, string>()
  const storage = {
    getItem: (k: string) => (mem.has(k) ? (mem.get(k) as string) : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
    key: (i: number) => Array.from(mem.keys())[i] ?? null,
    get length() {
      return mem.size
    },
  }
  ;(globalThis as { localStorage?: unknown }).localStorage = storage
  return { mem, storage, keychain: new Map<string, string>() }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === 'k2_secret_get') return h.keychain.get(`${args!.service}:${args!.account}`) ?? null
    return null
  }),
}))

import {
  CONNECT_HOSTS_STORAGE_KEY,
  K2_CONNECT_KEYCHAIN_SERVICE,
  attachConnectHostsStorageSync,
  duplicateSavedHost,
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from './connect-host'
import { onSavedHostRekey } from '@/lib/connect-host-hooks'
import { rekeyHostScopedStorage, SELECTED_TABS_STORAGE_KEY } from '@/lib/host-scoped-storage'
import { createHomesStore, rekeyHomeRows } from './homes'
import { verifyHostCredentials } from '@/lib/add-server-login'
import { createHostPool } from '@/lib/host-pool'

function host(over: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'id-b',
    label: 'Box B',
    hostname: 'b.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'tok-b',
    remember: true,
    lastConnectedAt: null,
    ...over,
  }
}

let rekeys: Array<[string, string]>
let offRekey: () => void

beforeEach(() => {
  h.mem.clear()
  h.keychain.clear()
  __resetConnectHostStoreForTests()
  rekeys = []
  offRekey?.()
  offRekey = onSavedHostRekey((o, n) => rekeys.push([o, n]))
})

describe('MS61 — one saved entry per server', () => {
  it('refuses a second entry for the same host key, in any spelling', () => {
    const s = useConnectHostStore.getState()
    expect(s.addHost(host({}))).toBe(true)
    expect(useConnectHostStore.getState().addHost(host({ id: 'id-b2', hostname: 'B.K2.DEV' }))).toBe(false)
    expect(useConnectHostStore.getState().hosts.map((x) => x.id)).toEqual(['id-b'])
    // http on 80 and https on 443 are different servers.
    expect(useConnectHostStore.getState().addHost(host({ id: 'id-b3', secure: false, port: 80 }))).toBe(true)
    expect(useConnectHostStore.getState().hosts.map((x) => x.id)).toEqual(['id-b', 'id-b3'])
  })

  it('refuses an edit that moves an entry onto another one’s address', () => {
    useConnectHostStore.getState().addHost(host({}))
    useConnectHostStore.getState().addHost(host({ id: 'id-c', hostname: 'c.k2.dev', label: 'Box C' }))
    expect(useConnectHostStore.getState().addHost(host({ id: 'id-c', hostname: 'b.k2.dev', label: 'Box C' }))).toBe(false)
    expect(useConnectHostStore.getState().hosts.find((x) => x.id === 'id-c')?.hostname).toBe('c.k2.dev')
    expect(rekeys).toEqual([])
  })

  it('keeps entries that were duplicates before the rule (a login can still save)', () => {
    useConnectHostStore.setState({ hosts: [host({}), host({ id: 'id-b-old', token: '' })] })
    expect(useConnectHostStore.getState().addHost(host({ id: 'id-b-old', token: 'tok-new' }))).toBe(true)
    expect(useConnectHostStore.getState().hosts.find((x) => x.id === 'id-b-old')?.token).toBe('tok-new')
  })

  it('the add-server flow surfaces "Already saved as …"', async () => {
    useConnectHostStore.getState().addHost(host({}))
    const login = vi.fn()
    const out = await verifyHostCredentials(host({ id: 'id-new', hostname: 'B.k2.dev' }), 'pw', true, {
      addHost: (x) => useConnectHostStore.getState().addHost(x),
      removeHost: (id) => useConnectHostStore.getState().removeHost(id),
      loginToHost: login,
    })
    expect(out).toEqual({ kind: 'error', reason: 'Already saved as Box B.' })
    expect(login).not.toHaveBeenCalled()
    expect(duplicateSavedHost(useConnectHostStore.getState().hosts, { hostname: 'b.k2.dev', port: 443, secure: true }, null)?.id).toBe('id-b')
  })
})

describe('MS61 — an address edit re-keys', () => {
  it('fires the re-key with the old and new host keys, and refreshes the active server', () => {
    const b = host({})
    useConnectHostStore.getState().addHost(b)
    useConnectHostStore.getState().selectHost(b)
    // The edit form keeps the live token on the copy it saves.
    expect(useConnectHostStore.getState().addHost(host({ hostname: 'b2.k2.dev', label: 'Box B2' }))).toBe(true)
    expect(rekeys).toEqual([['b.k2.dev', 'b2.k2.dev']])
    const active = useConnectHostStore.getState().activeHost
    if (active === 'local') throw new Error('window fell off its server')
    expect(active.hostname).toBe('b2.k2.dev')
    expect(active.token).toBe('tok-b')
    // A label-only edit is not a re-key.
    useConnectHostStore.getState().addHost(host({ hostname: 'b2.k2.dev', label: 'Renamed' }))
    expect(rekeys).toEqual([['b.k2.dev', 'b2.k2.dev']])
  })

  it('an edit of the active server without the token on the copy keeps the live session', () => {
    const b = host({})
    useConnectHostStore.getState().addHost(b)
    useConnectHostStore.getState().selectHost(b)
    useConnectHostStore.getState().addHost(host({ label: 'Renamed', token: '' }))
    const active = useConnectHostStore.getState().activeHost
    if (active === 'local') throw new Error('window fell off its server')
    expect(active.label).toBe('Renamed')
    expect(active.token).toBe('tok-b')
  })

  it('moves host-prefixed keys and selected tabs; an entry already under the new key wins', () => {
    h.storage.setItem('b.k2.dev|k2:composer:draft:p1', 'old draft')
    h.storage.setItem('b.k2.dev|k2.showLaunchBar', 'true')
    h.storage.setItem('b2.k2.dev|k2.showLaunchBar', 'false')
    h.storage.setItem('c.k2.dev|k2:composer:draft:p1', 'c draft')
    h.storage.setItem(
      SELECTED_TABS_STORAGE_KEY,
      JSON.stringify({ 'b.k2.dev|p1:w1': 'tab-1', 'c.k2.dev|p1:w1': 'tab-c' }),
    )
    const moved = rekeyHostScopedStorage(h.storage, 'b.k2.dev', 'b2.k2.dev')
    expect(moved).toBe(3)
    expect(h.storage.getItem('b.k2.dev|k2:composer:draft:p1')).toBeNull()
    expect(h.storage.getItem('b2.k2.dev|k2:composer:draft:p1')).toBe('old draft')
    expect(h.storage.getItem('b2.k2.dev|k2.showLaunchBar')).toBe('false')
    expect(h.storage.getItem('c.k2.dev|k2:composer:draft:p1')).toBe('c draft')
    expect(JSON.parse(h.storage.getItem(SELECTED_TABS_STORAGE_KEY) as string)).toEqual({
      'b2.k2.dev|p1:w1': 'tab-1',
      'c.k2.dev|p1:w1': 'tab-c',
    })
    // Idempotent: a second window runs it too.
    expect(rekeyHostScopedStorage(h.storage, 'b.k2.dev', 'b2.k2.dev')).toBe(0)
  })

  it('rewrites Home rows on the old key; a row already on the new address is dropped', () => {
    const store = createHomesStore({
      local: null,
      session: null,
      newId: (() => {
        let n = 0
        return () => `home-${++n}`
      })(),
      savedHosts: () => [],
    })
    const home = store.getState().homes[0]!
    store.getState().addRow(home.id, { address: 'bee::b.k2.dev', workspaceId: 'pb', label: 'Bee' })
    store.getState().addRow(home.id, { address: 'zed::b.k2.dev', workspaceId: 'pz', label: 'Zed' })
    store.getState().addRow(home.id, { address: 'zed::b2.k2.dev', workspaceId: 'pz', label: 'Zed' })
    store.getState().addRow(home.id, { address: 'cee::c.k2.dev', workspaceId: 'pc', label: 'Cee' })
    rekeyHomeRows(store, 'b.k2.dev', 'b2.k2.dev')
    const rows = store.getState().homes[0]!.rows.map((r) => r.address)
    expect(rows).toEqual(['bee::b2.k2.dev', 'zed::b2.k2.dev', 'cee::c.k2.dev'])
  })

  it('the pool forgets the old key’s entry', async () => {
    const pool = createHostPool({
      hosts: () => [host({})],
      windowHostKey: () => 'local',
      localCreds: async () => ({ base: 'http://127.0.0.1:1', token: 't' }),
      http: async () => new Response('{}', { status: 200 }),
      bootStatus: async () => ({ phase: 'ready', version: '0.41.6', protocol: 1, instanceId: 'i' }),
      resolvePassword: async () => null,
      login: async () => ({ ok: false, kind: 'server', reason: 'x' }),
      dropSessionInMemory: () => {},
      coord: {
        block: () => null,
        setBlock: () => {},
        clearBlock: () => {},
        leaseHeldElsewhere: () => false,
        tryBegin: async () => ({ ok: true }),
        end: () => {},
        recentAttempts: () => 0,
      },
      noteVersion: () => {},
      activate: async () => {
        throw new Error('projects/activate is not part of this test')
      },
      now: () => 1,
    })
    await pool.check('b.k2.dev')
    expect(pool.entry('b.k2.dev')?.reach).toBe('live')
    pool.forget('b.k2.dev')
    expect(pool.entry('b.k2.dev')).toBeUndefined()
  })
})

describe('MS62 — saved-server list across windows', () => {
  function storageEvent(newValue: string | null, key: string | null = CONNECT_HOSTS_STORAGE_KEY): Event {
    const e = new Event('storage') as Event & { key: string | null; newValue: string | null }
    Object.defineProperty(e, 'key', { value: key })
    Object.defineProperty(e, 'newValue', { value: newValue })
    return e
  }

  function persisted(list: ConnectHost[]): string {
    return JSON.stringify(list.map(({ token: _t, ...rest }) => rest))
  }

  it('merges another window’s list, keeps this window’s tokens, and re-keys an edit', () => {
    useConnectHostStore.setState({ hosts: [host({}), host({ id: 'id-c', hostname: 'c.k2.dev', token: 'tok-c' })] })
    const target = new EventTarget()
    const off = attachConnectHostsStorageSync(target, h.storage)
    target.dispatchEvent(
      storageEvent(persisted([host({ label: 'Box B (renamed)' }), host({ id: 'id-c', hostname: 'c2.k2.dev' })])),
    )
    off()
    const hosts = useConnectHostStore.getState().hosts
    expect(hosts.map((x) => [x.id, x.label, x.hostname, x.token])).toEqual([
      ['id-b', 'Box B (renamed)', 'b.k2.dev', 'tok-b'],
      ['id-c', 'Box B', 'c2.k2.dev', 'tok-c'],
    ])
    expect(rekeys).toEqual([['c.k2.dev', 'c2.k2.dev']])
    // No write-back: the other window already saved this list.
    expect(h.storage.getItem(CONNECT_HOSTS_STORAGE_KEY)).toBeNull()
  })

  it('a server saved in another window picks up its remembered token from the keychain', async () => {
    h.keychain.set(`${K2_CONNECT_KEYCHAIN_SERVICE}:id-n`, 'tok-n')
    const target = new EventTarget()
    const off = attachConnectHostsStorageSync(target, h.storage)
    target.dispatchEvent(storageEvent(persisted([host({ id: 'id-n', hostname: 'n.k2.dev' })])))
    off()
    expect(useConnectHostStore.getState().hosts[0]!.token).toBe('')
    await vi.waitFor(() => expect(useConnectHostStore.getState().hosts[0]!.token).toBe('tok-n'))
  })

  it('a removal elsewhere never moves this window off its server', () => {
    const b = host({})
    useConnectHostStore.getState().addHost(b)
    useConnectHostStore.getState().selectHost(b)
    const target = new EventTarget()
    const off = attachConnectHostsStorageSync(target, h.storage)
    target.dispatchEvent(storageEvent('[]'))
    off()
    expect(useConnectHostStore.getState().hosts).toEqual([])
    const active = useConnectHostStore.getState().activeHost
    if (active === 'local') throw new Error('window fell off its server')
    expect(active.id).toBe('id-b')
  })
})
