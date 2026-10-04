// @vitest-environment jsdom
//
// Home avatars (prd-home-picker-and-remote-avatars-v1 S4, tests T4.1–T4.6;
// vs-live P27, P28, P30, P33). The sync engine runs over fake deps that
// record every call: the local cache (get/put/prune) and each server's own
// `GET /cli/*`. The hook tests render real components over the real Homes,
// connect-host and pool stores.
//
// Fail loudly: every assertion names what it saw.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'

const web = vi.hoisted(() => ({ on: false }))
vi.mock('@/lib/is-web', () => ({ isWebClient: () => web.on }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  AVATAR_PRUNE_DELAY_MS,
  AVATAR_REFRESH_MS,
  avatarQueryBatches,
  createHomeAvatarSync,
  pruneKeepFrom,
  useHomeAvatarStore,
  useHomeAvatarSync,
  useHomeRowAvatar,
  type CachedAvatar,
  type HomeAvatarDeps,
  type HomeAvatarSync,
  type ListedAvatarWorkspace,
} from './home-avatars'
import { createHomesStore, useHomesStore, type HomeRow, type KeyValueStorage } from '@/stores/homes'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { hostPool, __resetHostPoolForTests } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { useProjectsStore } from '@/stores/projects'

const T0 = 1_800_000_000_000
const DAY = AVATAR_REFRESH_MS
const PNG_B1 = 'data:image/png;base64,iVBORw0KGgoAAAAB1'
const PNG_B2 = 'data:image/png;base64,iVBORw0KGgoAAAAB2'
const PNG_C = 'data:image/png;base64,iVBORw0KGgoAAAACC'
const PNG_LOCAL = 'data:image/png;base64,iVBORw0KGgoAAAALL'

interface Calls {
  hostGet: { host: string; route: string; params?: Record<string, string> }[]
  cacheGet: string[][]
  puts: { address: string; dataUrl: string | null }[]
  prunes: string[][]
}

/** Fake deps over an in-memory "disk" for the local cache. `serve` answers
 *  each server's GETs. */
function fakeDeps(serve: (host: string, route: string, params?: Record<string, string>) => unknown): {
  deps: HomeAvatarDeps
  calls: Calls
  disk: Map<string, CachedAvatar>
  clock: { now: number }
} {
  const calls: Calls = { hostGet: [], cacheGet: [], puts: [], prunes: [] }
  const disk = new Map<string, CachedAvatar>()
  const clock = { now: T0 }
  const deps: HomeAvatarDeps = {
    now: () => clock.now,
    isWeb: () => web.on,
    cacheGet: async (addresses) => {
      calls.cacheGet.push([...addresses])
      const out: Record<string, CachedAvatar> = {}
      for (const a of addresses) {
        const e = disk.get(a)
        if (e) out[a] = e
      }
      return out
    },
    cachePut: async (address, dataUrl) => {
      calls.puts.push({ address, dataUrl })
      disk.set(address, { dataUrl, missing: dataUrl === null, fetchedAt: clock.now, sha256: dataUrl ? 'h' : null })
    },
    cachePrune: async (keep) => {
      calls.prunes.push([...keep])
    },
    hostGet: async <T,>(host: string, route: string, params?: Record<string, string>): Promise<T> => {
      calls.hostGet.push(params ? { host, route, params } : { host, route })
      const r = serve(host, route, params)
      if (r instanceof Error) throw r
      return r as T
    },
  }
  return { deps, calls, disk, clock }
}

const B = 'b.k2.dev'
const C = 'c.k2.dev'
const bee: HomeRow = { address: `bee::${B}`, workspaceId: 'pb', label: 'Bee' }
const ant: HomeRow = { address: `ant::${B}`, workspaceId: 'pa', label: 'Ant' }
const cat: HomeRow = { address: `cat::${C}`, workspaceId: 'pc', label: 'Cat' }

const listB: ListedAvatarWorkspace[] = [
  { id: 'pb', name: 'Bee', handle: 'bee', path: '/srv/bee', iconUrl: PNG_B1 },
  { id: 'pa', name: 'Ant', handle: 'ant', path: '/srv/ant', iconUrl: PNG_B2 },
]

function liveEntry(hostKey: string, reach: HostEntry['reach'], auth: HostEntry['auth'] = 'ok'): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach,
    boot: null,
    auth,
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: reach === 'offline' ? 3 : 0,
    checkedAt: T0,
  }
}

const boxB: ConnectHost = {
  id: 'b',
  label: 'Box B',
  hostname: B,
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: true,
  lastConnectedAt: null,
}

beforeEach(() => {
  web.on = false
  useHomeAvatarStore.setState({ cached: {}, local: {} })
  __resetConnectHostStoreForTests()
  __resetHostPoolForTests()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

// ── The engine ──────────────────────────────────────────────────────────

describe('the sync engine', () => {
  it('T4.1: one projects/list to a live server covers all its rows; an offline server is never asked and keeps its image', async () => {
    const { deps, calls, disk, clock } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return listB
      throw new Error(`unexpected ${host} ${route}`)
    })
    disk.set(bee.address, { dataUrl: PNG_B1, missing: false, fetchedAt: T0 - DAY - 1, sha256: 'h' }) // stale
    disk.set(ant.address, { dataUrl: PNG_B2, missing: false, fetchedAt: T0 - 60_000, sha256: 'h' }) // fresh
    disk.set(cat.address, { dataUrl: PNG_C, missing: false, fetchedAt: T0 - 2 * DAY, sha256: 'h' }) // stale, offline
    const sync = createHomeAvatarSync(deps)
    clock.now = T0
    await sync.refresh({
      rows: [bee, ant, cat],
      connectedKey: 'local',
      connectedProjects: [],
      reachable: (k) => k === B,
    })
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
    expect(calls.puts.map((p) => p.address).sort()).toEqual([ant.address, bee.address].sort())
    expect(calls.puts.find((p) => p.address === bee.address)?.dataUrl).toBe(PNG_B1)
    expect(useHomeAvatarStore.getState().cached[cat.address]?.dataUrl).toBe(PNG_C)
    expect(useHomeAvatarStore.getState().cached[bee.address]?.fetchedAt).toBe(T0)
  })

  it('T4.2: a listing with no image asks get-icon on THAT server with the listed path; found:false records no image', async () => {
    const { deps, calls } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return [{ id: 'pb', name: 'Bee', handle: 'bee', path: '/srv/bee', iconUrl: null }]
      if (host === B && route === 'projects/get-icon') return { found: false, dataUrl: null }
      throw new Error(`unexpected ${host} ${route}`)
    })
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({ rows: [bee], connectedKey: 'local', connectedProjects: [], reachable: (k) => k === B })
    expect(calls.hostGet).toEqual([
      { host: B, route: 'projects/list' },
      { host: B, route: 'projects/get-icon', params: { path: '/srv/bee', project_id: 'pb' } },
    ])
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: null }])
    const e = useHomeAvatarStore.getState().cached[bee.address]
    expect(e?.missing).toBe(true)
    expect(e?.dataUrl).toBeNull()
  })

  it('an https:// iconUrl is not an image: get-icon is asked and nothing but a data URL is ever put', async () => {
    const { deps, calls } = fakeDeps((host, route) => {
      if (route === 'projects/list') return [{ id: 'pb', name: 'Bee', handle: 'bee', path: '/srv/bee', iconUrl: 'https://evil.example/x.png' }]
      if (route === 'projects/get-icon') return { found: true, dataUrl: 'https://evil.example/y.png' }
      throw new Error(`unexpected ${host} ${route}`)
    })
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({ rows: [bee], connectedKey: 'local', connectedProjects: [], reachable: () => true })
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: null }])
  })

  it('a refusal from the cache (too large, odd type) records the row as having no image instead of retrying forever', async () => {
    const { deps, calls } = fakeDeps((_h, route) => (route === 'projects/list' ? listB.slice(0, 1) : null))
    const realPut = deps.cachePut
    deps.cachePut = async (address, dataUrl) => {
      if (dataUrl !== null) {
        calls.puts.push({ address, dataUrl })
        throw new Error('too_large')
      }
      await realPut(address, dataUrl)
    }
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({ rows: [bee], connectedKey: 'local', connectedProjects: [], reachable: () => true })
    expect(calls.puts).toEqual([
      { address: bee.address, dataUrl: PNG_B1 },
      { address: bee.address, dataUrl: null },
    ])
    expect(useHomeAvatarStore.getState().cached[bee.address]?.missing).toBe(true)
  })

  it('P12a: rows on the connected server come from the projects store with no network call', async () => {
    const { deps, calls } = fakeDeps((h, r) => new Error(`no network expected: ${h} ${r}`))
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({
      rows: [bee, ant],
      connectedKey: B,
      connectedProjects: [listB[0], { ...listB[1], iconUrl: null }],
      reachable: () => false,
    })
    expect(calls.hostGet).toEqual([])
    // Ant has no image in the store: nothing is said about it yet.
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: PNG_B1 }])
  })

  it('an unreadable cache asks no server at all', async () => {
    const { deps, calls } = fakeDeps(() => listB)
    deps.cacheGet = async () => {
      throw new Error('route_unclassified')
    }
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({ rows: [bee], connectedKey: 'local', connectedProjects: [], reachable: () => true })
    expect(calls.hostGet).toEqual([])
    expect(calls.puts).toEqual([])
  })

  it('T4.6: a `local` row on a remote window reads this computer’s list, then get-icon with the listed path, and puts nothing', async () => {
    const { deps, calls } = fakeDeps((host, route) => {
      if (host === 'local' && route === 'projects/list') return [{ id: 'pl', name: 'Lo', handle: 'lo', path: '/Users/me/lo', iconUrl: null }]
      if (host === 'local' && route === 'projects/get-icon') return { found: true, dataUrl: PNG_LOCAL }
      throw new Error(`unexpected ${host} ${route}`)
    })
    const sync = createHomeAvatarSync(deps)
    const lo: HomeRow = { address: 'lo::local', workspaceId: 'pl', label: 'Lo' }
    await sync.readLocalRows([lo, bee], B)
    expect(calls.hostGet).toEqual([
      { host: 'local', route: 'projects/list' },
      { host: 'local', route: 'projects/get-icon', params: { path: '/Users/me/lo', project_id: 'pl' } },
    ])
    expect(calls.puts).toEqual([])
    expect(useHomeAvatarStore.getState().local[lo.address]).toBe(PNG_LOCAL)
    // On the local window itself, `local` rows are the connected server: nothing to read.
    calls.hostGet.length = 0
    await sync.readLocalRows([lo], 'local')
    expect(calls.hostGet).toEqual([])
    // refresh() never caches `local` rows.
    await sync.refresh({ rows: [lo], connectedKey: B, connectedProjects: [], reachable: () => true })
    expect(calls.hostGet).toEqual([])
    expect(calls.puts).toEqual([])
  })

  it('P33: a put from the picker’s listing needs no list call', async () => {
    const { deps, calls } = fakeDeps((h, r) => new Error(`no network expected: ${h} ${r}`))
    const sync = createHomeAvatarSync(deps)
    await sync.putFromListing(B, bee.address, listB[0])
    expect(calls.hostGet).toEqual([])
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: PNG_B1 }])
    expect(useHomeAvatarStore.getState().cached[bee.address]?.dataUrl).toBe(PNG_B1)
  })

  it('T4.5: on the web client nothing is read, fetched, put or pruned', async () => {
    web.on = true
    const { deps, calls } = fakeDeps(() => listB)
    const sync = createHomeAvatarSync(deps)
    await sync.readCache([bee.address])
    await sync.refresh({ rows: [bee], connectedKey: 'local', connectedProjects: [], reachable: () => true })
    await sync.readLocalRows([{ address: 'lo::local', workspaceId: null, label: 'Lo' }], B)
    await sync.putFromListing(B, bee.address, listB[0])
    sync.schedulePrune(() => [bee.address])
    await sync.flushPrune()
    expect(calls).toEqual({ hostGet: [], cacheGet: [], puts: [], prunes: [] })
  })

  it('P30: cache reads are split into ≤ 100 addresses and ≤ 8 KiB of query', () => {
    const many = Array.from({ length: 250 }, (_, i) => `w${i}::${B}`)
    const batches = avatarQueryBatches(many)
    expect(batches.map((b) => b.length)).toEqual([100, 100, 50])
    expect(batches.flat()).toEqual(many)
    const long = Array.from({ length: 60 }, (_, i) => `${'h'.repeat(180)}${i}::[fe80::1]:38471`)
    for (const b of avatarQueryBatches(long)) {
      expect(encodeURIComponent(b.join(',')).length).toBeLessThanOrEqual(8 * 1024)
    }
    expect(avatarQueryBatches(long).flat()).toEqual(long)
  })
})

// ── The hooks ───────────────────────────────────────────────────────────

function Row({ address }: { address: string }): React.JSX.Element {
  const url = useHomeRowAvatar(address)
  return (
    <ProjectAvatar
      projectPath={`home:${address}`}
      projectName={address}
      projectColor="#777"
      iconUrl={url}
      size={20}
      fetchIcon={false}
    />
  )
}

function Effects({ sync }: { sync: HomeAvatarSync }): null {
  useHomeAvatarSync(sync)
  return null
}

function setHome(rows: HomeRow[], extra: { id: string; name: string; rows: HomeRow[] }[] = []): void {
  useHomesStore.setState({
    homes: [{ id: 'h1', name: 'Home', rows }, ...extra],
    selectedId: 'h1',
    storageOk: true,
  })
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) {
    await act(async () => {
      await Promise.resolve()
    })
  }
}

describe('the hooks', () => {
  it('rows paint from the cache with no server call on any render, and refresh once after a day', async () => {
    useConnectHostStore.setState({ hosts: [boxB], connectionStatus: 'connected' })
    hostPool.store.setState({ entries: { [B]: liveEntry(B, 'live') } })
    const { deps, calls, disk, clock } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return listB
      throw new Error(`unexpected ${host} ${route}`)
    })
    disk.set(bee.address, { dataUrl: PNG_B1, missing: false, fetchedAt: T0 - 1000, sha256: 'h' })
    disk.set(ant.address, { dataUrl: null, missing: true, fetchedAt: T0 - 1000, sha256: null })
    setHome([bee, ant])
    const sync = createHomeAvatarSync(deps)

    const view = render(
      <>
        <Effects sync={sync} />
        <Row address={bee.address} />
        <Row address={ant.address} />
      </>,
    )
    await settle()
    const imgs = (): string[] => Array.from(view.container.querySelectorAll('img')).map((i) => i.getAttribute('src') ?? '')
    expect(imgs()).toEqual([PNG_B1])
    expect(view.container.textContent).toContain('A') // Ant has no image: its letter
    expect(calls.hostGet).toEqual([])
    expect(calls.cacheGet).toEqual([[bee.address, ant.address]])

    for (let i = 0; i < 20; i++) {
      view.rerender(
        <>
          <Effects sync={sync} />
          <Row address={bee.address} />
          <Row address={ant.address} />
        </>,
      )
    }
    // A pool check that changes nothing about reach/login.
    act(() => hostPool.store.setState({ entries: { [B]: { ...liveEntry(B, 'live'), checkedAt: T0 + 30_000 } } }))
    await settle()
    expect(calls.hostGet).toEqual([])

    // A day later, the window comes back into focus: one list, both rows put.
    clock.now = T0 + DAY + 1
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await settle()
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
    expect(calls.puts.map((p) => p.address).sort()).toEqual([ant.address, bee.address].sort())
    expect(imgs()).toEqual([PNG_B1, PNG_B2])

    // Focus again the same day: nothing more is asked.
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await settle()
    expect(calls.hostGet).toHaveLength(1)
  })

  it('fetch on add: a new row on a live server is listed once and paints', async () => {
    useConnectHostStore.setState({ hosts: [boxB], connectionStatus: 'connected' })
    hostPool.store.setState({ entries: { [B]: liveEntry(B, 'live') } })
    const { deps, calls } = fakeDeps((host, route) => (host === B && route === 'projects/list' ? listB : new Error(route)))
    setHome([])
    const sync = createHomeAvatarSync(deps)
    const view = render(
      <>
        <Effects sync={sync} />
        <Row address={bee.address} />
      </>,
    )
    await settle()
    expect(calls.hostGet).toEqual([])
    act(() => {
      useHomesStore.getState().addRow('h1', bee)
    })
    await settle()
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
    expect(view.container.querySelector('img')?.getAttribute('src')).toBe(PNG_B1)
  })

  it('a server that needs a sign-in is not asked; its login landing asks once', async () => {
    useConnectHostStore.setState({ hosts: [boxB], connectionStatus: 'connected' })
    hostPool.store.setState({ entries: { [B]: liveEntry(B, 'live', 'signin-required') } })
    const { deps, calls } = fakeDeps((host, route) => (host === B && route === 'projects/list' ? listB : new Error(route)))
    setHome([bee])
    const sync = createHomeAvatarSync(deps)
    render(<Effects sync={sync} />)
    await settle()
    expect(calls.hostGet).toEqual([])
    act(() => hostPool.store.setState({ entries: { [B]: liveEntry(B, 'live', 'ok') } }))
    await settle()
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
  })

  it('T4.6 (hook): a `local` row on a remote window paints this computer’s image, read once, with no put', async () => {
    useConnectHostStore.setState({ hosts: [boxB], activeHost: boxB, connectionStatus: 'connected' })
    const lo: HomeRow = { address: 'lo::local', workspaceId: 'pl', label: 'Lo' }
    const { deps, calls } = fakeDeps((host, route) => {
      if (host === 'local' && route === 'projects/list') return [{ id: 'pl', name: 'Lo', handle: 'lo', path: '/Users/me/lo', iconUrl: PNG_LOCAL }]
      throw new Error(`unexpected ${host} ${route}`)
    })
    setHome([lo])
    const sync = createHomeAvatarSync(deps)
    const view = render(
      <>
        <Effects sync={sync} />
        <Row address={lo.address} />
      </>,
    )
    await settle()
    expect(view.container.querySelector('img')?.getAttribute('src')).toBe(PNG_LOCAL)
    expect(calls.hostGet).toEqual([{ host: 'local', route: 'projects/list' }])
    // The connected (remote) server's project list churning asks this computer nothing more.
    for (let i = 0; i < 5; i++) act(() => useProjectsStore.setState({ projects: [] }))
    await settle()
    expect(calls.hostGet).toHaveLength(1)
    expect(calls.puts).toEqual([])
  })

  it('T4.3: removing the last row for an address prunes once within 2.5 s, keeping every Home’s addresses', async () => {
    vi.useFakeTimers()
    const { deps, calls } = fakeDeps(() => new Error('no network in this test'))
    setHome([bee, ant], [{ id: 'h2', name: 'Two', rows: [ant, cat] }])
    const sync = createHomeAvatarSync(deps)
    render(<Effects sync={sync} />)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_PRUNE_DELAY_MS + 500)
    })
    expect(calls.prunes).toEqual([[ant.address, bee.address, cat.address].sort()])

    act(() => {
      useHomesStore.getState().removeRow('h1', bee.address)
      useHomesStore.getState().removeRow('h1', ant.address) // still on Two
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_PRUNE_DELAY_MS + 500)
    })
    expect(calls.prunes).toHaveLength(2)
    expect(calls.prunes[1]).toEqual([ant.address, cat.address].sort())

    // A change that leaves the address set alone (a rename) prunes nothing more.
    act(() => {
      useHomesStore.getState().renameHome('h2', 'Renamed')
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_PRUNE_DELAY_MS + 500)
    })
    expect(calls.prunes).toHaveLength(2)
  })

  it('P28: a Homes store that fell back to its in-memory seed never prunes', async () => {
    vi.useFakeTimers()
    const throwing: KeyValueStorage = {
      getItem: () => {
        throw new Error('storage is locked')
      },
      setItem: () => {
        throw new Error('storage is locked')
      },
    }
    let n = 0
    const fallback = createHomesStore({ local: throwing, session: null, newId: () => `id-${++n}` })
    expect(fallback.getState().storageOk).toBe(false)
    const homeId = fallback.getState().homes[0].id
    fallback.getState().addRow(homeId, bee)
    fallback.getState().removeRow(homeId, bee.address)
    expect(pruneKeepFrom(fallback.getState())).toBeNull()

    // The hook over a store in that state makes zero prune calls.
    const { deps, calls } = fakeDeps(() => new Error('no network in this test'))
    useHomesStore.setState({ homes: [{ id: 'h1', name: 'Home', rows: [bee] }], selectedId: 'h1', storageOk: false })
    render(<Effects sync={createHomeAvatarSync(deps)} />)
    act(() => useHomesStore.getState().removeRow('h1', bee.address))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_PRUNE_DELAY_MS * 3)
    })
    expect(calls.prunes).toEqual([])
  })

  it('P28: storageOk is true for a parsed doc, a written first-run seed, and an applied external doc', () => {
    const mem = new Map<string, string>()
    const kv: KeyValueStorage = { getItem: (k) => mem.get(k) ?? null, setItem: (k, v) => void mem.set(k, v) }
    let n = 0
    const first = createHomesStore({ local: kv, session: null, newId: () => `id-${++n}` })
    expect(first.getState().storageOk).toBe(true)
    const again = createHomesStore({ local: kv, session: null, newId: () => `id-${++n}` })
    expect(again.getState().storageOk).toBe(true)
    const junk = createHomesStore({
      local: { getItem: () => '{"version":9}', setItem: () => undefined },
      session: null,
      newId: () => `id-${++n}`,
    })
    expect(junk.getState().storageOk).toBe(false)
    junk.getState().applyExternal(mem.get('k2.homes.v1') ?? null)
    expect(junk.getState().storageOk).toBe(true)
  })

  it('T4.5: on the web client the row hook returns null even with a cached image', () => {
    useHomeAvatarStore.setState({ cached: { [bee.address]: { dataUrl: PNG_B1, missing: false, fetchedAt: T0, sha256: 'h' } }, local: {} })
    const on = render(<Row address={bee.address} />)
    expect(on.container.querySelector('img')?.getAttribute('src')).toBe(PNG_B1)
    cleanup()
    web.on = true
    const off = render(<Row address={bee.address} />)
    expect(off.container.querySelector('img')).toBeNull()
  })

  it('T4.4: ProjectAvatar with fetchIcon off goes from a data URL back to its letter when the image goes away', () => {
    const view = render(
      <ProjectAvatar projectPath="home:bee::b" projectName="Bee" projectColor="#777" iconUrl={PNG_B1} fetchIcon={false} />,
    )
    expect(view.container.querySelector('img')?.getAttribute('src')).toBe(PNG_B1)
    view.rerender(<ProjectAvatar projectPath="home:bee::b" projectName="Bee" projectColor="#777" iconUrl={null} fetchIcon={false} />)
    expect(view.container.querySelector('img')).toBeNull()
    expect(view.container.textContent).toBe('B')
  })
})
