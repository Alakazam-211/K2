// Home P1 — the named Homes store (vs-live H11/H13/H19) + row addresses
// (H14). Pure: every test builds its own store over in-memory storages, so
// "two windows" is two stores sharing one localStorage with their own
// sessionStorage.

import { describe, it, expect, vi } from 'vitest'
import {
  createHomesStore,
  attachHomesStorageSync,
  parseHomesDoc,
  selectedHome,
  HOMES_STORAGE_KEY,
  HOMES_LAST_SELECTED_KEY,
  HOMES_WINDOW_SELECTED_KEY,
  type KeyValueStorage,
  type HomeRow,
} from './homes'
import {
  homeAddress,
  homeHostKey,
  parseHomeAddress,
  findWorkspaceForRow,
  savedHostForKey,
  workspaceHandle,
} from '@/lib/home-address'
import { formatAgentHost } from '@/lib/federation'
import type { ConnectHost } from './connect-host'

class MemStorage implements KeyValueStorage {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
}

class ThrowingStorage implements KeyValueStorage {
  getItem(): string | null {
    throw new Error('SecurityError: storage disabled')
  }
  setItem(): void {
    throw new Error('QuotaExceededError')
  }
}

function ids(): () => string {
  let n = 0
  return () => `h${++n}`
}

function row(address: string, workspaceId: string | null = null, label = address.split('::')[0]): HomeRow {
  return { address, workspaceId, label }
}

function stored(local: MemStorage): { version: number; homes: { id: string; name: string; rows: HomeRow[] }[] } {
  const raw = local.getItem(HOMES_STORAGE_KEY)
  if (raw === null) throw new Error('k2.homes.v1 was never written')
  return JSON.parse(raw)
}

function storageEvent(key: string | null, newValue: string | null): Event {
  return Object.assign(new Event('storage'), { key, newValue })
}

describe('homes store — first run, CRUD, last Home', () => {
  it('first run seeds one empty "Home" and saves it', () => {
    const local = new MemStorage()
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    expect(s.getState().homes).toEqual([{ id: 'h1', name: 'Home', rows: [] }])
    expect(s.getState().selectedId).toBe('h1')
    expect(stored(local)).toEqual({ version: 1, homes: [{ id: 'h1', name: 'Home', rows: [] }] })
  })

  it('create selects the new Home; rename trims; empty names are refused', () => {
    const local = new MemStorage()
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    const id = s.getState().createHome('  Sales   team ')
    expect(id).toBe('h2')
    expect(s.getState().selectedId).toBe('h2')
    expect(selectedHome(s.getState()).name).toBe('Sales team')

    expect(s.getState().createHome('   ')).toBeNull()
    expect(s.getState().homes).toHaveLength(2)

    expect(s.getState().renameHome('h2', 'Ops')).toBe(true)
    expect(s.getState().renameHome('h2', '')).toBe(false)
    expect(s.getState().renameHome('nope', 'X')).toBe(false)
    expect(stored(local).homes.map((h) => h.name)).toEqual(['Home', 'Ops'])
  })

  it('the last Home cannot be deleted; deleting the selected one selects a neighbour', () => {
    const local = new MemStorage()
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    expect(s.getState().deleteHome('h1')).toBe(false)
    expect(s.getState().homes).toHaveLength(1)

    s.getState().createHome('Two')
    s.getState().createHome('Three')
    expect(s.getState().selectedId).toBe('h3')
    expect(s.getState().deleteHome('h3')).toBe(true)
    expect(s.getState().selectedId).toBe('h2')
    expect(s.getState().deleteHome('missing')).toBe(false)
    expect(s.getState().deleteHome('h1')).toBe(true)
    expect(s.getState().homes.map((h) => h.id)).toEqual(['h2'])
    expect(s.getState().deleteHome('h2')).toBe(false)
    expect(stored(local).homes.map((h) => h.id)).toEqual(['h2'])
  })
})

describe('homes store — rows', () => {
  it('dedupes per Home (case-insensitive address), not across Homes', () => {
    const local = new MemStorage()
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    const two = s.getState().createHome('Two') as string

    expect(s.getState().addRow('h1', row('cortana::local', 'p1'))).toBe(true)
    expect(s.getState().addRow('h1', row('Cortana::LOCAL', 'p1'))).toBe(false)
    expect(s.getState().homes[0].rows).toEqual([row('cortana::local', 'p1')])

    // Same agent on a second Home.
    expect(s.getState().addRow(two, row('cortana::local', 'p1'))).toBe(true)
    expect(s.getState().homes[1].rows).toHaveLength(1)

    // Removing from one Home leaves the other.
    s.getState().removeRow('h1', 'cortana::local')
    expect(s.getState().homes[0].rows).toEqual([])
    expect(s.getState().homes[1].rows).toEqual([row('cortana::local', 'p1')])
    expect(stored(local).homes[1].rows).toEqual([row('cortana::local', 'p1')])
  })

  it('refuses a malformed address and an unknown Home', () => {
    const s = createHomesStore({ local: new MemStorage(), session: new MemStorage(), newId: ids() })
    expect(s.getState().addRow('h1', row('no-host'))).toBe(false)
    expect(s.getState().addRow('zzz', row('a::local'))).toBe(false)
    expect(s.getState().homes[0].rows).toEqual([])
  })

  it('moves rows within a Home', () => {
    const s = createHomesStore({ local: new MemStorage(), session: new MemStorage(), newId: ids() })
    for (const a of ['a::local', 'b::local', 'c::local']) s.getState().addRow('h1', row(a))
    s.getState().moveRow('h1', 0, 2)
    expect(s.getState().homes[0].rows.map((r) => r.address)).toEqual(['b::local', 'c::local', 'a::local'])
    s.getState().moveRow('h1', 5, 0) // out of range: no-op
    expect(s.getState().homes[0].rows.map((r) => r.address)).toEqual(['b::local', 'c::local', 'a::local'])
  })

  it('repairRows rewrites a renamed handle on every Home and never duplicates', () => {
    const local = new MemStorage()
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    const two = s.getState().createHome('Two') as string
    s.getState().addRow('h1', row('old::box.k2.dev', 'p9', 'Old'))
    s.getState().addRow(two, row('old::box.k2.dev', 'p9', 'Old'))
    s.getState().addRow(two, row('new::box.k2.dev', 'p9', 'New'))

    s.getState().repairRows((r) =>
      r.workspaceId === 'p9' ? { address: 'new::box.k2.dev', workspaceId: 'p9', label: 'New' } : null,
    )
    expect(s.getState().homes[0].rows).toEqual([row('new::box.k2.dev', 'p9', 'New')])
    expect(s.getState().homes[1].rows).toEqual([row('new::box.k2.dev', 'p9', 'New')])
    expect(stored(local).homes[0].rows).toEqual([row('new::box.k2.dev', 'p9', 'New')])
  })

  it('repairRows with nothing to change does not write', () => {
    const local = new MemStorage()
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    s.getState().addRow('h1', row('a::local', 'p1', 'A'))
    const before = local.getItem(HOMES_STORAGE_KEY)
    const setItem = vi.spyOn(local, 'setItem')
    s.getState().repairRows((r) => ({ ...r }))
    expect(setItem).not.toHaveBeenCalled()
    expect(local.getItem(HOMES_STORAGE_KEY)).toBe(before)
  })
})

describe('homes store — windows', () => {
  it('each window keeps its own selection; a new window opens on the last one used', () => {
    const local = new MemStorage()
    const w1 = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    w1.getState().createHome('Two') // h2, selected in w1
    const w2Session = new MemStorage()
    const w2 = createHomesStore({ local, session: w2Session, newId: ids() })
    expect(w2.getState().selectedId).toBe('h2') // lastSelected

    w2.getState().selectHome('h1')
    expect(w2.getState().selectedId).toBe('h1')
    expect(w1.getState().selectedId).toBe('h2') // w1 unchanged
    expect(local.getItem(HOMES_LAST_SELECTED_KEY)).toBe('h1')
    expect(w2Session.getItem(HOMES_WINDOW_SELECTED_KEY)).toBe('h1')

    // w1 reloads (same sessionStorage): keeps ITS pick, not the last used.
    const w1Session = new MemStorage()
    w1Session.setItem(HOMES_WINDOW_SELECTED_KEY, 'h2')
    const w1Reloaded = createHomesStore({ local, session: w1Session, newId: ids() })
    expect(w1Reloaded.getState().selectedId).toBe('h2')

    // A third, brand-new window opens on the last one used.
    const w3 = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    expect(w3.getState().selectedId).toBe('h1')
  })

  it('cross-window sync through the storage event', () => {
    const local = new MemStorage()
    const w1 = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    const w2 = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    const target = new EventTarget()
    const detach = attachHomesStorageSync(w2, target, local)

    w1.getState().addRow('h1', row('anna::dtl.k2.dev', 'p2', 'Anna'))
    expect(w2.getState().homes[0].rows).toEqual([]) // not yet delivered
    target.dispatchEvent(storageEvent(HOMES_STORAGE_KEY, local.getItem(HOMES_STORAGE_KEY)))
    expect(w2.getState().homes[0].rows).toEqual([row('anna::dtl.k2.dev', 'p2', 'Anna')])

    // Unrelated keys are ignored; garbage never replaces good state.
    target.dispatchEvent(storageEvent('k2.connect-hosts.v1', '[]'))
    target.dispatchEvent(storageEvent(HOMES_STORAGE_KEY, '{not json'))
    expect(w2.getState().homes[0].rows).toEqual([row('anna::dtl.k2.dev', 'p2', 'Anna')])

    // The other window deletes the Home w2 has selected → w2 falls back.
    const two = w1.getState().createHome('Two') as string
    target.dispatchEvent(storageEvent(HOMES_STORAGE_KEY, local.getItem(HOMES_STORAGE_KEY)))
    w2.getState().selectHome(two)
    w1.getState().deleteHome(two)
    target.dispatchEvent(storageEvent(HOMES_STORAGE_KEY, local.getItem(HOMES_STORAGE_KEY)))
    expect(w2.getState().homes.map((h) => h.id)).toEqual(['h1'])
    expect(w2.getState().selectedId).toBe('h1')

    detach()
    w1.getState().createHome('After detach')
    target.dispatchEvent(storageEvent(HOMES_STORAGE_KEY, local.getItem(HOMES_STORAGE_KEY)))
    expect(w2.getState().homes).toHaveLength(1)
  })
})

describe('homes store — storage trouble', () => {
  it('a storage error falls back to an in-memory seed that still works', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const s = createHomesStore({ local: new ThrowingStorage(), session: new ThrowingStorage(), newId: ids() })
    expect(s.getState().homes).toEqual([{ id: 'h1', name: 'Home', rows: [] }])
    expect(s.getState().addRow('h1', row('a::local'))).toBe(true)
    expect(s.getState().createHome('Two')).toBe('h2')
    expect(s.getState().selectedId).toBe('h2')
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })

  it('unreadable data is not overwritten at boot', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const local = new MemStorage()
    local.setItem(HOMES_STORAGE_KEY, JSON.stringify({ version: 2, homes: [{ id: 'x' }] }))
    const s = createHomesStore({ local, session: new MemStorage(), newId: ids() })
    expect(s.getState().homes).toEqual([{ id: 'h1', name: 'Home', rows: [] }])
    expect(local.getItem(HOMES_STORAGE_KEY)).toBe(JSON.stringify({ version: 2, homes: [{ id: 'x' }] }))
    warn.mockRestore()
  })

  it('parseHomesDoc accepts only a version-1 doc with valid rows', () => {
    expect(parseHomesDoc(null)).toBeNull()
    expect(parseHomesDoc('[]')).toBeNull()
    expect(parseHomesDoc(JSON.stringify({ version: 1, homes: [] }))).toBeNull()
    expect(
      parseHomesDoc(JSON.stringify({ version: 1, homes: [{ id: 'a', name: 'A', rows: [{ address: 'bad' }] }] })),
    ).toBeNull()
    const good = { version: 1, homes: [{ id: 'a', name: 'A', rows: [row('x::local', null, 'X')] }] }
    expect(parseHomesDoc(JSON.stringify(good))).toEqual(good)
  })
})

describe('home row addresses (H14)', () => {
  const lan: Pick<ConnectHost, 'hostname' | 'port' | 'secure'> = { hostname: '192.168.1.20', port: 38471, secure: false }
  const hosted: Pick<ConnectHost, 'hostname' | 'port' | 'secure'> = { hostname: 'Rosson.K2.dev', port: 443, secure: true }

  it('host key: local, default port dropped, LAN keeps ip:port', () => {
    expect(homeHostKey('local')).toBe('local')
    expect(homeHostKey(hosted)).toBe('rosson.k2.dev')
    expect(homeHostKey(lan)).toBe('192.168.1.20:38471')
    expect(homeHostKey({ hostname: 'box.k2.dev', port: 8443, secure: true })).toBe('box.k2.dev:8443')
  })

  it('address is formatAgentHost and round-trips, including local and ip:port', () => {
    for (const host of ['local', 'rosson.k2.dev', '192.168.1.20:38471']) {
      const a = homeAddress('Cortana', host)
      expect(a).toBe(formatAgentHost('Cortana', host))
      expect(parseHomeAddress(a)).toEqual({ handle: 'cortana', host })
    }
    expect(parseHomeAddress('nohost')).toBeNull()
    expect(parseHomeAddress('::local')).toBeNull()
    expect(parseHomeAddress('a::b::c')).toBeNull()
  })

  it('finds by handle first, then repairs by id; handle falls back to the name slug', () => {
    const list = [
      { id: 'p1', name: 'Cortana', handle: 'cortana' },
      { id: 'p2', name: 'Anna Bot', handle: '' },
    ]
    expect(workspaceHandle(list[1])).toBe('anna-bot')
    expect(findWorkspaceForRow(list, { address: 'anna-bot::local', workspaceId: null })?.id).toBe('p2')
    // Handle moved: the id finds it.
    expect(findWorkspaceForRow(list, { address: 'old-name::local', workspaceId: 'p1' })?.id).toBe('p1')
    // Handle wins over a stale id.
    expect(findWorkspaceForRow(list, { address: 'cortana::local', workspaceId: 'p2' })?.id).toBe('p1')
    expect(findWorkspaceForRow(list, { address: 'gone::local', workspaceId: 'p404' })).toBeNull()
  })

  it('matches a saved server by hostname + port, preferring one with a login', () => {
    const base = { label: 'Box', port: 443, secure: true, remember: true, lastConnectedAt: null }
    const hosts: ConnectHost[] = [
      { ...base, id: 'a', hostname: 'box.k2.dev', token: '' },
      { ...base, id: 'b', hostname: 'BOX.k2.dev', token: 'tok' },
      { ...base, id: 'c', hostname: '10.0.0.5', port: 38471, secure: false, token: '' },
    ]
    expect(savedHostForKey(hosts, 'box.k2.dev')?.id).toBe('b')
    expect(savedHostForKey(hosts, '10.0.0.5:38471')?.id).toBe('c')
    expect(savedHostForKey(hosts, '10.0.0.5')).toBeNull()
  })
})
