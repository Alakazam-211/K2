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
  AVATAR_MISSING_RECHECK_MS,
  AVATAR_PRUNE_DELAY_MS,
  AVATAR_RECHECK_FLOOR_MS,
  AVATAR_RECHECK_TICK_MS,
  AVATAR_REFRESH_MS,
  avatarEntryStale,
  parseAgentColors,
  missingRecheckMs,
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
import { agentLookColor } from '@/lib/agent-look'
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
  colorSaves: string[]
}

/** Fake deps over an in-memory "disk" for the local cache. `serve` answers
 *  each server's GETs. */
function fakeDeps(serve: (host: string, route: string, params?: Record<string, string>) => unknown): {
  deps: HomeAvatarDeps
  calls: Calls
  disk: Map<string, CachedAvatar>
  clock: { now: number }
  /** The stored agent colors (raw), as localStorage would hold them. */
  colorStore: { raw: string | null }
} {
  const calls: Calls = { hostGet: [], cacheGet: [], puts: [], prunes: [], colorSaves: [] }
  const colorStore: { raw: string | null } = { raw: null }
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
    colorsLoad: () => colorStore.raw,
    colorsSave: (raw) => {
      calls.colorSaves.push(raw)
      colorStore.raw = raw
    },
  }
  return { deps, calls, disk, clock, colorStore }
}

const B = 'b.k2.dev'
const C = 'c.k2.dev'
const bee: HomeRow = { address: `bee::${B}`, workspaceId: 'pb', label: 'Bee' }
const ant: HomeRow = { address: `ant::${B}`, workspaceId: 'pa', label: 'Ant' }
const cat: HomeRow = { address: `cat::${C}`, workspaceId: 'pc', label: 'Cat' }

const BEE_COLOR = '#d97706'
const ANT_COLOR = '#2563eb'
const listB: ListedAvatarWorkspace[] = [
  { id: 'pb', name: 'Bee', handle: 'bee', path: '/srv/bee', color: BEE_COLOR, iconUrl: PNG_B1 },
  { id: 'pa', name: 'Ant', handle: 'ant', path: '/srv/ant', color: ANT_COLOR, iconUrl: PNG_B2 },
]

/** The stored agent colors, as `colorsSave` writes them. Tests about the
 *  picture rules start with the colors already known, so a row is never
 *  due for a colors-only listing there. */
function storedColors(colors: Record<string, string>): string {
  return JSON.stringify({ version: 1, colors })
}

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
  useHomeAvatarStore.setState({ cached: {}, local: {}, colors: {} })
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
    expect(calls).toEqual({ hostGet: [], cacheGet: [], puts: [], prunes: [], colorSaves: [] })
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

// ── A miss lives short (Appa 10-28) ─────────────────────────────────────
//
// A peer with no picture yet was cached `missing` under the 24 h image rule,
// so a picture it set an hour later stayed a letter all day. A miss is now
// re-checked after 10 min, then 30 min, then hourly; an explicit ask or the
// peer's identity event re-checks it at once (never twice inside 5 s).

const MIN = 60_000

describe('missing rows', () => {
  /** B lists Bee with no picture until `pic.on`, then with PNG_B1; get-icon
   *  agrees. */
  function peerB(): ReturnType<typeof fakeDeps> & { pic: { on: boolean }; sync: HomeAvatarSync } {
    const pic = { on: false }
    const f = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return [{ ...listB[0], iconUrl: pic.on ? PNG_B1 : null }]
      if (host === B && route === 'projects/get-icon') {
        return pic.on ? { found: true, dataUrl: PNG_B1 } : { found: false, dataUrl: null }
      }
      throw new Error(`unexpected ${host} ${route}`)
    })
    f.colorStore.raw = storedColors({ [bee.address]: BEE_COLOR })
    return { ...f, pic, sync: createHomeAvatarSync(f.deps) }
  }
  const input = { rows: [bee], connectedKey: 'local', connectedProjects: [], reachable: (k: string) => k === B }
  const lists = (calls: Calls): number => calls.hostGet.filter((c) => c.route === 'projects/list').length

  it('the backoff is 10 min, 30 min, then hourly; a miss not yet re-checked starts at 10 min', () => {
    expect(AVATAR_MISSING_RECHECK_MS).toEqual([10 * MIN, 30 * MIN, 60 * MIN])
    expect([0, 1, 2, 3, 4, 50].map(missingRecheckMs)).toEqual([
      10 * MIN,
      10 * MIN,
      30 * MIN,
      60 * MIN,
      60 * MIN,
      60 * MIN,
    ])
  })

  it('avatarEntryStale: a miss past its life is due, a fresh one is not; an image keeps the 24 h rule', () => {
    const miss = (age: number) => ({ missing: true, fetchedAt: T0 - age })
    const img = (age: number) => ({ missing: false, fetchedAt: T0 - age })
    expect(avatarEntryStale(miss(10 * MIN + 1), T0, 0, false)).toBe(true)
    expect(avatarEntryStale(miss(10 * MIN - 1), T0, 0, false)).toBe(false)
    expect(avatarEntryStale(miss(10 * MIN + 1), T0, 2, false)).toBe(false) // second miss: 30 min
    expect(avatarEntryStale(miss(30 * MIN + 1), T0, 2, false)).toBe(true)
    expect(avatarEntryStale(miss(30 * MIN + 1), T0, 3, false)).toBe(false) // third+: hourly
    expect(avatarEntryStale(miss(60 * MIN + 1), T0, 9, false)).toBe(true)
    expect(avatarEntryStale(img(10 * MIN + 1), T0, 0, false)).toBe(false)
    expect(avatarEntryStale(img(23 * 60 * MIN), T0, 0, false)).toBe(false)
    expect(avatarEntryStale(img(DAY + 1), T0, 0, false)).toBe(true)
    // Forced: due once past the floor, never inside it.
    expect(avatarEntryStale(miss(AVATAR_RECHECK_FLOOR_MS - 1), T0, 0, true)).toBe(false)
    expect(avatarEntryStale(miss(AVATAR_RECHECK_FLOOR_MS), T0, 0, true)).toBe(true)
    expect(avatarEntryStale(undefined, T0, 0, false)).toBe(true)
  })

  it('a missing marker older than 10 min is refetched, and the picture the peer set since lands', async () => {
    const { calls, disk, sync, pic } = peerB()
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: T0 - 10 * MIN - 1, sha256: null })
    pic.on = true
    await sync.refresh(input)
    expect(lists(calls)).toBe(1)
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: PNG_B1 }])
    expect(useHomeAvatarStore.getState().cached[bee.address]?.dataUrl).toBe(PNG_B1)
  })

  it('a missing marker younger than 10 min is not refetched', async () => {
    const { calls, disk, sync, pic } = peerB()
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: T0 - 10 * MIN + 1, sha256: null })
    pic.on = true
    await sync.refresh(input)
    expect(calls.hostGet).toEqual([])
    expect(calls.puts).toEqual([])
    expect(useHomeAvatarStore.getState().cached[bee.address]?.missing).toBe(true)
  })

  it('repeated misses back off: 10 min, 30 min, then hourly, never sooner', async () => {
    const { calls, clock, sync } = peerB()
    await sync.refresh(input) // first look: no picture (miss 1)
    expect(lists(calls)).toBe(1)
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: null }])
    let last = clock.now
    /** Refresh `ms` after the last fetch; was B listed? */
    const at = async (ms: number): Promise<boolean> => {
      const before = lists(calls)
      clock.now = last + ms
      await sync.refresh(input)
      const asked = lists(calls) > before
      if (asked) last = clock.now
      return asked
    }
    expect(await at(10 * MIN - 1)).toBe(false)
    expect(await at(10 * MIN + 1)).toBe(true) // miss 2 → 30 min
    expect(await at(10 * MIN + 1)).toBe(false)
    expect(await at(30 * MIN - 1)).toBe(false)
    expect(await at(30 * MIN + 1)).toBe(true) // miss 3 → hourly
    expect(await at(30 * MIN + 1)).toBe(false)
    expect(await at(60 * MIN + 1)).toBe(true)
    expect(await at(30 * MIN + 1)).toBe(false)
    expect(await at(60 * MIN + 1)).toBe(true) // stays hourly
    expect(calls.puts.map((p) => p.dataUrl)).toEqual([null, null, null, null, null])
  })

  it('an image resets the backoff, keeps the 24 h rule, and a later miss starts at 10 min again', async () => {
    const { calls, clock, sync, pic } = peerB()
    await sync.refresh(input) // miss 1
    clock.now += 10 * MIN + 1
    await sync.refresh(input) // miss 2 → 30 min
    pic.on = true
    clock.now += 30 * MIN + 1
    await sync.refresh(input) // image
    expect(calls.puts.at(-1)).toEqual({ address: bee.address, dataUrl: PNG_B1 })
    const listed = lists(calls)
    expect(listed).toBe(3)
    clock.now += 23 * 60 * MIN
    await sync.refresh(input)
    expect(lists(calls)).toBe(listed)
    pic.on = false
    clock.now += 60 * MIN + 1 // past 24 h since the image
    await sync.refresh(input) // miss 1 again
    expect(calls.puts.at(-1)).toEqual({ address: bee.address, dataUrl: null })
    clock.now += 10 * MIN + 1
    await sync.refresh(input)
    expect(lists(calls)).toBe(listed + 2)
  })

  it('an image the cache refuses counts as a miss, so it backs off instead of retrying every 10 min', async () => {
    const { deps, calls, clock } = fakeDeps((_h, route) => (route === 'projects/list' ? listB.slice(0, 1) : null))
    const realPut = deps.cachePut
    deps.cachePut = async (address, dataUrl) => {
      if (dataUrl !== null) {
        calls.puts.push({ address, dataUrl })
        throw new Error('too_large')
      }
      await realPut(address, dataUrl)
    }
    const sync = createHomeAvatarSync(deps)
    await sync.refresh(input) // miss 1
    clock.now += 10 * MIN + 1
    await sync.refresh(input) // miss 2 → 30 min
    expect(lists(calls)).toBe(2)
    clock.now += 10 * MIN + 1
    await sync.refresh(input)
    expect(lists(calls)).toBe(2)
  })

  it('an image entry follows the 24 h rule: never refetched inside a day, refetched after', async () => {
    const { calls, disk, clock, sync, pic } = peerB()
    pic.on = true
    disk.set(bee.address, { dataUrl: PNG_B2, missing: false, fetchedAt: T0, sha256: 'h' })
    for (const ms of [10 * MIN + 1, 60 * MIN + 1, DAY - 1]) {
      clock.now = T0 + ms
      await sync.refresh(input)
      expect(calls.hostGet).toEqual([])
    }
    clock.now = T0 + DAY + 1
    await sync.refresh(input)
    expect(lists(calls)).toBe(1)
    expect(useHomeAvatarStore.getState().cached[bee.address]?.dataUrl).toBe(PNG_B1)
  })

  it('a user ask re-checks a fresh miss at once (past the 5 s floor), never an image, and repeated asks do not loop', async () => {
    const { calls, clock, sync, pic, disk } = peerB()
    await sync.refresh(input) // miss, fetched at T0
    expect(lists(calls)).toBe(1)
    pic.on = true
    clock.now = T0 + AVATAR_RECHECK_FLOOR_MS - 1
    await sync.recheck([bee.address], 'user')
    expect(lists(calls)).toBe(1) // inside the floor: the ask is kept for later
    clock.now = T0 + AVATAR_RECHECK_FLOOR_MS
    await sync.refresh(input)
    expect(lists(calls)).toBe(2) // the kept ask runs on the next refresh
    expect(useHomeAvatarStore.getState().cached[bee.address]?.dataUrl).toBe(PNG_B1)
    // An image is not re-checked by a user ask.
    clock.now += 60 * MIN
    await sync.recheck([bee.address], 'user')
    expect(lists(calls)).toBe(2)
    // Back to a miss: one ask runs, a burst of clicks after it asks nobody.
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: clock.now, sha256: null })
    await sync.readCache([bee.address])
    pic.on = false
    clock.now += AVATAR_RECHECK_FLOOR_MS
    await sync.recheck([bee.address], 'user')
    expect(lists(calls)).toBe(3)
    for (let i = 0; i < 10; i++) {
      clock.now += 100
      await sync.recheck([bee.address], 'user')
    }
    expect(lists(calls)).toBe(3)
  })

  it('a peer identity change (image) refetches an image row at once', async () => {
    const { calls, disk, clock, sync, pic } = peerB()
    disk.set(bee.address, { dataUrl: PNG_B2, missing: false, fetchedAt: T0, sha256: 'h' })
    await sync.refresh(input) // a fresh image: nothing asked; the input is recorded
    expect(calls.hostGet).toEqual([])
    pic.on = true
    clock.now = T0 + AVATAR_RECHECK_FLOOR_MS
    await sync.recheck([bee.address], 'changed')
    expect(lists(calls)).toBe(1)
    expect(useHomeAvatarStore.getState().cached[bee.address]?.dataUrl).toBe(PNG_B1)
  })

  it('recheckDue reads nothing when nothing is due, and re-reads the cache before asking a peer', async () => {
    const { calls, disk, clock, sync } = peerB()
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: T0, sha256: null })
    await sync.refresh(input)
    const reads = calls.cacheGet.length
    await sync.recheckDue(input)
    expect(calls.cacheGet.length).toBe(reads)
    // Due, but another window re-checked a moment ago: read, not asked.
    clock.now = T0 + 10 * MIN + 1
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: clock.now - 1000, sha256: null })
    await sync.recheckDue(input)
    expect(calls.cacheGet.length).toBe(reads + 1)
    expect(calls.hostGet).toEqual([])
    // Due again: read, asked, and read back after the put.
    clock.now += 10 * MIN + 1
    await sync.recheckDue(input)
    expect(calls.cacheGet.slice(reads + 1)).toEqual([[bee.address], [bee.address]])
    expect(lists(calls)).toBe(1)
  })

  it('the connected server: a row the cache calls missing takes the store’s new picture with no wait and no network', async () => {
    const { deps, calls, disk } = fakeDeps((h, r) => new Error(`no network expected: ${h} ${r}`))
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: T0 - 1000, sha256: null })
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({ rows: [bee], connectedKey: B, connectedProjects: [listB[0]], reachable: () => false })
    expect(calls.hostGet).toEqual([])
    expect(calls.puts).toEqual([{ address: bee.address, dataUrl: PNG_B1 }])
  })
})

// ── Agent colors (Rosson 10-30) ─────────────────────────────────────────
//
// A row on another server keeps its agent's own color. The color rides the
// listings this module already reads; only hex survives validation; a row
// whose color was never seen is listed once per window for colors alone.

describe('agent colors', () => {
  const input = { rows: [bee, ant], connectedKey: 'local', connectedProjects: [], reachable: (k: string) => k === B }
  const fresh = (disk: Map<string, CachedAvatar>): void => {
    disk.set(bee.address, { dataUrl: PNG_B1, missing: false, fetchedAt: T0 - 1000, sha256: 'h' })
    disk.set(ant.address, { dataUrl: PNG_B2, missing: false, fetchedAt: T0 - 1000, sha256: 'h' })
  }

  it('the check a listed color passes (agentLookColor) keeps only #rgb / #rrggbb hex', () => {
    expect(agentLookColor('#D97706')).toBe('#d97706')
    expect(agentLookColor('d97706')).toBe('#d97706')
    expect(agentLookColor(' #AbC ')).toBe('#aabbcc')
    for (const bad of [
      'red',
      'rgb(1, 2, 3)',
      'var(--color-text-muted)',
      '#12345',
      '#1234567',
      '#d97706; background-image: url(https://evil.example/x)',
      'url(javascript:alert(1))',
      '',
      null,
      undefined,
    ]) {
      expect([bad, agentLookColor(bad)]).toEqual([bad, null])
    }
  })

  it('parseAgentColors drops junk: wrong version, non-hex colors, non-address keys', () => {
    expect(parseAgentColors(null)).toEqual({})
    expect(parseAgentColors('not json')).toEqual({})
    expect(parseAgentColors(JSON.stringify({ version: 2, colors: { [bee.address]: '#123456' } }))).toEqual({})
    expect(
      parseAgentColors(
        JSON.stringify({
          version: 1,
          colors: { [bee.address]: '#ABCDEF', [ant.address]: 'red', 'no-host': '#123456' },
        }),
      ),
    ).toEqual({ [bee.address]: '#abcdef' })
  })

  it('a row whose color was never seen: its server is listed once for colors only (no get-icon, no put), then never again this window', async () => {
    const { deps, calls, disk, colorStore } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return listB
      throw new Error(`unexpected ${host} ${route}`)
    })
    fresh(disk)
    const sync = createHomeAvatarSync(deps)
    await sync.refresh(input)
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
    expect(calls.puts).toEqual([])
    expect(useHomeAvatarStore.getState().colors).toEqual({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR })
    expect(parseAgentColors(colorStore.raw)).toEqual({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR })
    await sync.refresh(input)
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
  })

  it('a stored color is read with the cache and asks nobody; an offline server keeps its row’s stored color', async () => {
    const { deps, calls, disk, colorStore } = fakeDeps((h, r) => new Error(`no network expected: ${h} ${r}`))
    fresh(disk)
    disk.set(cat.address, { dataUrl: PNG_C, missing: false, fetchedAt: T0 - 1000, sha256: 'h' })
    colorStore.raw = storedColors({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR, [cat.address]: '#0f0' })
    const sync = createHomeAvatarSync(deps)
    await sync.refresh({ ...input, rows: [bee, ant, cat] })
    expect(calls.hostGet).toEqual([])
    expect(useHomeAvatarStore.getState().colors[cat.address]).toBe('#00ff00')
    expect(calls.colorSaves).toEqual([])
  })

  it('a server that lists a non-hex color gets nothing into the store, and is not asked again for it this window', async () => {
    const { deps, calls, disk } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') {
        return [
          { ...listB[0], color: 'red; background-image: url(https://evil.example/x)' },
          { ...listB[1], color: 'var(--color-accent)' },
        ]
      }
      throw new Error(`unexpected ${host} ${route}`)
    })
    fresh(disk)
    const sync = createHomeAvatarSync(deps)
    await sync.refresh(input)
    expect(useHomeAvatarStore.getState().colors).toEqual({})
    await sync.refresh(input)
    expect(calls.hostGet.length).toBe(1)
  })

  it('a stale picture’s listing refreshes the colors too: a recolor there lands here', async () => {
    const { deps, disk, colorStore } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return [{ ...listB[0], color: '#FF0000' }, listB[1]]
      throw new Error(`unexpected ${host} ${route}`)
    })
    fresh(disk)
    disk.set(bee.address, { dataUrl: PNG_B1, missing: false, fetchedAt: T0 - DAY - 1, sha256: 'h' })
    colorStore.raw = storedColors({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR })
    const sync = createHomeAvatarSync(deps)
    await sync.refresh(input)
    expect(useHomeAvatarStore.getState().colors[bee.address]).toBe('#ff0000')
    expect(parseAgentColors(colorStore.raw)[bee.address]).toBe('#ff0000')
  })

  it('the Add Agent picker’s listing records the color on add; an open room’s list records a recolor', async () => {
    const { deps, calls } = fakeDeps((h, r) => new Error(`no network expected: ${h} ${r}`))
    const sync = createHomeAvatarSync(deps)
    await sync.putFromListing(B, bee.address, listB[0])
    expect(useHomeAvatarStore.getState().colors[bee.address]).toBe(BEE_COLOR)
    expect(calls.hostGet).toEqual([])
    // A room on B sees `agent_identity_changed` (color): its list is patched
    // and handed over; rows on other servers are untouched.
    sync.noteListColors(B, [{ ...listB[0], color: '#00AA00' }], [bee, cat])
    expect(useHomeAvatarStore.getState().colors).toEqual({ [bee.address]: '#00aa00' })
  })

  it('prune drops the colors of rows on no Home', async () => {
    const { deps, colorStore } = fakeDeps(() => new Error('no network'))
    colorStore.raw = storedColors({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR })
    const sync = createHomeAvatarSync(deps)
    await sync.readCache([bee.address])
    sync.schedulePrune(() => [bee.address])
    await sync.flushPrune()
    expect(useHomeAvatarStore.getState().colors).toEqual({ [bee.address]: BEE_COLOR })
    expect(parseAgentColors(colorStore.raw)).toEqual({ [bee.address]: BEE_COLOR })
  })

  it('storage that throws: colors still paint from this window’s listing, nothing breaks', async () => {
    const { deps, disk } = fakeDeps((host, route) => (host === B && route === 'projects/list' ? listB : new Error(route)))
    deps.colorsLoad = () => {
      throw new Error('SecurityError: storage is disabled')
    }
    deps.colorsSave = () => {
      throw new Error('QuotaExceededError')
    }
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    fresh(disk)
    const sync = createHomeAvatarSync(deps)
    await sync.refresh(input)
    expect(useHomeAvatarStore.getState().colors).toEqual({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR })
    expect(warn.mock.calls.map((c) => String(c[0]))).toEqual([
      '[home-avatars] could not read the stored agent colors:',
      '[home-avatars] could not store the agent colors:',
    ])
    warn.mockRestore()
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
    const { deps, calls, disk, clock, colorStore } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return listB
      throw new Error(`unexpected ${host} ${route}`)
    })
    disk.set(bee.address, { dataUrl: PNG_B1, missing: false, fetchedAt: T0 - 1000, sha256: 'h' })
    disk.set(ant.address, { dataUrl: null, missing: true, fetchedAt: T0 - 1000, sha256: null })
    colorStore.raw = storedColors({ [bee.address]: BEE_COLOR, [ant.address]: ANT_COLOR })
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

  it('on screen with no focus change, a missing row is re-checked by the tick once 10 min have passed (Appa 10-28)', async () => {
    vi.useFakeTimers()
    useConnectHostStore.setState({ hosts: [boxB], connectionStatus: 'connected' })
    hostPool.store.setState({ entries: { [B]: liveEntry(B, 'live') } })
    const { deps, calls, disk, clock, colorStore } = fakeDeps((host, route) => {
      if (host === B && route === 'projects/list') return listB.slice(0, 1)
      throw new Error(`unexpected ${host} ${route}`)
    })
    disk.set(bee.address, { dataUrl: null, missing: true, fetchedAt: T0, sha256: null })
    colorStore.raw = storedColors({ [bee.address]: BEE_COLOR })
    setHome([bee])
    const sync = createHomeAvatarSync(deps)
    const view = render(
      <>
        <Effects sync={sync} />
        <Row address={bee.address} />
      </>,
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(calls.hostGet).toEqual([])
    expect(view.container.querySelector('img')).toBeNull()
    // A tick before the miss is due asks nobody.
    clock.now = T0 + AVATAR_RECHECK_TICK_MS
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_RECHECK_TICK_MS)
    })
    expect(calls.hostGet).toEqual([])
    // The first tick past 10 min lists B once and the picture paints.
    clock.now = T0 + 10 * MIN + 1
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_RECHECK_TICK_MS)
    })
    expect(calls.hostGet).toEqual([{ host: B, route: 'projects/list' }])
    expect(view.container.querySelector('img')?.getAttribute('src')).toBe(PNG_B1)
    // Later ticks on an image ask nobody.
    clock.now += 60 * MIN
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AVATAR_RECHECK_TICK_MS * 3)
    })
    expect(calls.hostGet).toHaveLength(1)
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
