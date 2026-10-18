// homes-sync-v1 — the pure merge engine: diff, pull, ack, rebuild and the
// first-sync union, with the laptop/phone address mapping.

import { describe, it, expect } from 'vitest'
import type { Home } from '@/stores/homes'
import {
  ackPush,
  applyPull,
  beats,
  buildLocal,
  diffLocal,
  emptySyncState,
  firstSync,
  itemKey,
  parsePullAnswer,
  parseSyncState,
  resolveRedirect,
  type AddressMap,
  type PullAnswer,
  type SyncState,
  type WireItem,
} from './engine'

const SID = 'aaaaaaaaaaaaaaaa'
const OTHER = 'bbbbbbbbbbbbbbbb'

/** The laptop: `local` is the home server itself. */
const laptop: AddressMap = {
  toWire: (a) => a.replace(/::local$/, `::${SID}`).replace(/::z3mbpz\.k2\.dev$/, `::${OTHER}`),
  fromWire: (a) => a.replace(new RegExp(`::${SID}$`), '::local').replace(new RegExp(`::${OTHER}$`), '::z3mbpz.k2.dev'),
}

/** The phone: it knows the same box as `rosson.k2.dev`, and not z3mbpZ. */
const phone: AddressMap = {
  toWire: (a) => a.replace(/::rosson\.k2\.dev$/, `::${SID}`),
  fromWire: (a) => a.replace(new RegExp(`::${SID}$`), '::rosson.k2.dev'),
}

function ctx(map: AddressMap, deviceId: string, now: number) {
  return { ...map, deviceId, now }
}

function home(id: string, name: string, rows: string[] = []): Home {
  return { id, name, rows: rows.map((address) => ({ address, workspaceId: null, label: address.split('::')[0] })) }
}

function pull(changes: WireItem[], cursor: number, extra: Partial<PullAnswer> = {}): PullAnswer {
  return { cursor, full: false, changes, redirects: {}, serverId: SID, ...extra }
}

function merged(deviceId: string): SyncState {
  return { ...emptySyncState(deviceId), merged: true, serverId: SID, cursor: 0 }
}

describe('homes-sync engine', () => {
  it('last writer wins per item; ties go to the larger deviceId; a tombstone wins a full tie', () => {
    const a: WireItem = { kind: 'home', id: 'h', name: 'A', order: 'V', updatedAt: 10, deviceId: 'a' }
    expect(beats({ ...a, updatedAt: 11 }, a)).toBe(true)
    expect(beats(a, { ...a, updatedAt: 11 })).toBe(false)
    expect(beats({ ...a, deviceId: 'b' }, a)).toBe(true)
    expect(beats(a, { ...a, deviceId: 'b' })).toBe(false)
    expect(beats({ ...a, deleted: true }, a)).toBe(true)
    expect(beats(a, { ...a })).toBe(false)
  })

  it('diffLocal queues new, renamed and reordered items and tombstones what left; label repairs never queue', () => {
    let st = merged('lap')
    const homes = [home('h1', 'Home', ['atlas::local', 'pi::z3mbpz.k2.dev'])]
    let r = diffLocal(homes, st, ctx(laptop, 'lap', 100))
    expect(r.changes.map(itemKey).sort()).toEqual([`h|h1`, `r|h1|atlas::${SID}`, `r|h1|pi::${OTHER}`].sort())
    expect(r.changes.every((c) => c.deviceId === 'lap' && c.updatedAt === 100)).toBe(true)
    st = r.state
    expect(Object.keys(st.outbox)).toHaveLength(3)
    // Nothing changed: nothing queued.
    expect(diffLocal(homes, st, ctx(laptop, 'lap', 101)).changes).toEqual([])
    // A label / workspaceId repair stays local.
    const relabelled = [{ ...homes[0], rows: homes[0].rows.map((x) => ({ ...x, label: 'Atlas 2', workspaceId: 'w-9' })) }]
    expect(diffLocal(relabelled, st, ctx(laptop, 'lap', 102)).changes).toEqual([])
    // A move rewrites only the moved row's order.
    const moved = [{ ...homes[0], rows: [homes[0].rows[1], homes[0].rows[0]] }]
    r = diffLocal(moved, st, ctx(laptop, 'lap', 103))
    expect(r.changes).toHaveLength(1)
    expect(r.changes[0].kind).toBe('row')
    // A rename and a removal.
    const renamed = [{ ...moved[0], name: 'Desk', rows: [moved[0].rows[0]] }]
    r = diffLocal(renamed, r.state, ctx(laptop, 'lap', 104))
    const byKey = Object.fromEntries(r.changes.map((c) => [itemKey(c), c]))
    expect((byKey['h|h1'] as { name: string }).name).toBe('Desk')
    expect(byKey[`r|h1|atlas::${SID}`]?.deleted).toBe(true)
    // A local edit is stamped past what it was made on, whatever the clock says.
    const skewed = diffLocal([{ ...renamed[0], name: 'Again' }], r.state, ctx(laptop, 'lap', 1))
    expect(skewed.changes[0].updatedAt).toBe(105)
  })

  it('applyPull: newer server items replace (and drop a beaten local edit); older ones lose to the queued edit', () => {
    let st = merged('lap')
    st = diffLocal([home('h1', 'Laptop name')], st, ctx(laptop, 'lap', 100)).state
    // The phone's newer rename wins and clears the outbox entry.
    let next = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'Phone name', order: 'V', updatedAt: 200, deviceId: 'ph' }], 5), 300)
    expect((next.known['h|h1'] as { name: string }).name).toBe('Phone name')
    expect(next.outbox['h|h1']).toBeUndefined()
    expect(next.cursor).toBe(5)
    // An older server version loses: the local edit stays queued.
    next = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'Old', order: 'V', updatedAt: 50, deviceId: 'ph' }], 6), 300)
    expect((next.known['h|h1'] as { name: string }).name).toBe('Laptop name')
    expect(next.outbox['h|h1']).toBeDefined()
  })

  it('delete vs edit: a newer tombstone removes the row locally; an edit after the delete brings it back', () => {
    let st = merged('lap')
    st = diffLocal([home('h1', 'Home', ['atlas::local'])], st, ctx(laptop, 'lap', 100)).state
    st = ackPush(st, Object.values(st.outbox), []).state
    const tomb: WireItem = { kind: 'row', homeId: 'h1', address: `atlas::${SID}`, label: 'atlas', workspaceId: null, order: 'V', updatedAt: 150, deviceId: 'ph', deleted: true }
    st = applyPull(st, pull([tomb], 2), 200)
    expect(buildLocal(st, [], laptop)).toEqual([{ id: 'h1', name: 'Home', rows: [] }])
    // A stale copy of the row (older than the tombstone) can't resurrect it.
    st = applyPull(st, pull([{ ...tomb, deleted: undefined, updatedAt: 120 }], 3), 200)
    expect(buildLocal(st, [], laptop)?.[0].rows).toEqual([])
    // An edit newer than the delete brings it back.
    st = applyPull(st, pull([{ ...tomb, deleted: undefined, updatedAt: 160, order: 'k' }], 4), 200)
    expect(buildLocal(st, [], laptop)?.[0].rows.map((r) => r.address)).toEqual(['atlas::local'])
  })

  it('a full pull forgets live items the server no longer has, unless they are queued here', () => {
    let st = merged('lap')
    st = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'A', order: 'V', updatedAt: 1, deviceId: 'x' }, { kind: 'home', id: 'h2', name: 'B', order: 'l', updatedAt: 1, deviceId: 'x' }], 1), 10)
    st = diffLocal([home('h1', 'A'), home('h2', 'B'), home('h3', 'New')], st, ctx(laptop, 'lap', 20)).state
    const next = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'A', order: 'V', updatedAt: 1, deviceId: 'x' }], 9, { full: true }), 30)
    expect(next.known['h|h2']).toBeUndefined()
    expect(next.known['h|h3']).toBeDefined()
  })

  it('first sync combines both lists: by Home name and row, server ids survive, local ids redirect, nothing deleted', () => {
    // The server already holds the phone's Homes.
    const server: WireItem[] = [
      { kind: 'home', id: 'srv-home', name: 'Home', order: 'V', updatedAt: 1, deviceId: 'ph' },
      { kind: 'home', id: 'srv-work', name: 'Work', order: 'l', updatedAt: 1, deviceId: 'ph' },
      { kind: 'row', homeId: 'srv-home', address: `atlas::${SID}`, label: 'Atlas', workspaceId: null, order: 'V', updatedAt: 1, deviceId: 'ph' },
    ]
    const local = [
      home('lap-home', ' home ', ['atlas::local', 'nova::local']),
      home('lap-side', 'Side', ['pi::z3mbpz.k2.dev']),
    ]
    const { state, changes } = firstSync(local, pull(server, 3, { full: true }), emptySyncState('lap'), ctx(laptop, 'lap', 500))
    expect(state.merged).toBe(true)
    expect(state.cursor).toBe(3)
    expect(state.redirects).toEqual({ 'lap-home': 'srv-home' })
    const keys = changes.map(itemKey).sort()
    expect(keys).toEqual(['h|lap-home', 'h|lap-side', `r|lap-side|pi::${OTHER}`, `r|srv-home|nova::${SID}`].sort())
    const tomb = changes.find((c) => itemKey(c) === 'h|lap-home') as { deleted?: boolean; mergedInto?: string }
    expect(tomb.deleted).toBe(true)
    expect(tomb.mergedInto).toBe('srv-home')
    // Atlas (the same agent: `local` on the laptop = SID) is not duplicated.
    expect(changes.some((c) => itemKey(c) === `r|srv-home|atlas::${SID}`)).toBe(false)
    const built = buildLocal(state, local, laptop)
    expect(built?.map((h) => [h.id, h.name, h.rows.map((r) => r.address)])).toEqual([
      ['srv-home', 'Home', ['atlas::local', 'nova::local']],
      ['srv-work', 'Work', []],
      ['lap-side', 'Side', ['pi::z3mbpz.k2.dev']],
    ])
    // The rebuilt copy diffs clean: no churn after the union.
    expect(diffLocal(built as Home[], state, ctx(laptop, 'lap', 501)).changes).toEqual([])
    // Redirects resolve (and survive chains and loops).
    expect(resolveRedirect({ a: 'b', b: 'c' }, 'a')).toBe('c')
    expect(resolveRedirect({ a: 'b', b: 'a' }, 'a')).toBe('a')
  })

  it('first sync against an empty server pushes the whole local copy', () => {
    const local = [home('h1', 'Home', ['atlas::local']), home('h2', 'Work')]
    const { state, changes } = firstSync(local, pull([], 0, { full: true }), emptySyncState('lap'), ctx(laptop, 'lap', 10))
    expect(changes.map(itemKey).sort()).toEqual(['h|h1', 'h|h2', `r|h1|atlas::${SID}`].sort())
    const orders = changes.filter((c) => c.kind === 'home').map((c) => c.order)
    expect(orders[0] < orders[1]).toBe(true)
    expect(buildLocal(state, local, laptop)?.map((h) => h.id)).toEqual(['h1', 'h2'])
  })

  it('rows name the server by serverId: the laptop’s local and the phone’s rosson.k2.dev are one row', () => {
    const lap = diffLocal([home('h1', 'Home', ['atlas::local'])], merged('lap'), ctx(laptop, 'lap', 10))
    const wire = lap.changes.find((c) => c.kind === 'row') as WireItem
    expect((wire as { address: string }).address).toBe(`atlas::${SID}`)
    // The phone renders it as its own host key for that box…
    const ph = applyPull(merged('ph'), pull(lap.changes, 1), 20)
    expect(buildLocal(ph, [], phone)?.[0].rows.map((r) => r.address)).toEqual(['atlas::rosson.k2.dev'])
    // …and a row on a server the phone hasn't saved stays as its id (never dropped).
    const other = applyPull(ph, pull([{ kind: 'row', homeId: 'h1', address: `pi::${OTHER}`, label: 'Pi', workspaceId: null, order: 'z', updatedAt: 30, deviceId: 'lap' }], 2), 40)
    expect(buildLocal(other, [], phone)?.[0].rows.map((r) => r.address)).toEqual(['atlas::rosson.k2.dev', `pi::${OTHER}`])
    // Rebuilt rows diff clean on the phone too (the unknown id round-trips).
    const built = buildLocal(other, [], phone) as Home[]
    expect(diffLocal(built, other, ctx(phone, 'ph', 50)).changes).toEqual([])
  })

  it('hostKey → serverId migration: a row pushed by host key moves to the id once the id is known', () => {
    // A device that didn't know z3mbpZ's id pushed `pi::z3mbpz.k2.dev`.
    let st = applyPull(merged('lap'), pull([
      { kind: 'home', id: 'h1', name: 'Home', order: 'V', updatedAt: 1, deviceId: 'old' },
      { kind: 'row', homeId: 'h1', address: 'pi::z3mbpz.k2.dev', label: 'Pi', workspaceId: 'w-1', order: 'V', updatedAt: 1, deviceId: 'old' },
    ], 1), 5)
    const built = buildLocal(st, [], laptop) as Home[]
    expect(built[0].rows[0].address).toBe('pi::z3mbpz.k2.dev')
    // The laptop knows that host's id: the row is re-keyed (add the id row, tombstone the host-key row).
    const r = diffLocal(built, st, ctx(laptop, 'lap', 10))
    const byKey = Object.fromEntries(r.changes.map((c) => [itemKey(c), c]))
    expect(byKey[`r|h1|pi::${OTHER}`]?.deleted).toBeUndefined()
    expect((byKey[`r|h1|pi::${OTHER}`] as { label: string; order: string }).label).toBe('Pi')
    expect(byKey['r|h1|pi::z3mbpz.k2.dev']?.deleted).toBe(true)
    st = r.state
    // While both are live somewhere, the local copy shows one row.
    expect((buildLocal(st, built, laptop) as Home[])[0].rows).toHaveLength(1)
  })

  it('ackPush drops only the version that was sent; a refused item is forgotten and asks for a full pull', () => {
    let st = diffLocal([home('h1', 'Home')], merged('lap'), ctx(laptop, 'lap', 10)).state
    const sent = Object.values(st.outbox)
    // Renamed again while the push was in flight: the newer edit stays queued.
    st = diffLocal([home('h1', 'Home 2')], st, ctx(laptop, 'lap', 20)).state
    let acked = ackPush(st, sent, [])
    expect(Object.keys(acked.state.outbox)).toEqual(['h|h1'])
    expect(acked.needsFullPull).toBe(false)
    acked = ackPush(st, Object.values(st.outbox), [{ item: { ...st.outbox['h|h1'] } }])
    expect(acked.state.outbox['h|h1']).toBeUndefined()
    expect(acked.state.known['h|h1']).toBeUndefined()
    expect(acked.needsFullPull).toBe(true)
  })

  it('parses stored state and server answers defensively', () => {
    expect(parseSyncState(null)).toBeNull()
    expect(parseSyncState('{"version":2,"deviceId":"x"}')).toBeNull()
    expect(parseSyncState('not json')).toBeNull()
    const st = parseSyncState(JSON.stringify({ version: 1, deviceId: 'd', cursor: 4, merged: true }))
    expect(st?.cursor).toBe(4)
    expect(st?.outbox).toEqual({})
    const p = parsePullAnswer({ cursor: 2, full: true, changes: [{ kind: 'home', id: 'h', name: 'H', order: 'V', updatedAt: 1 }, { junk: 1 }], redirects: { a: 'b', bad: 3 }, serverId: SID })
    expect(p.changes).toHaveLength(1)
    expect(p.redirects).toEqual({ a: 'b' })
    expect(() => parsePullAnswer({ changes: [] })).toThrow(/cursor/)
  })
})
