// homes-sync-v1 (0.46.1) — the pure merge engine for a device's Homes.
//
// Local-first (Rosson, ticket 4f537560): the device's own `k2.homes.v1` is
// the working copy and the only thing the UI reads. The home server is the
// sync peer. This module holds no timers and does no I/O: the runner
// (`runner.ts`) feeds it the local Homes and the server's answers and
// persists what it returns.
//
// Wire items (the contract K2 Companion speaks too):
//   Home: {kind:'home', id, name, order, updatedAt, deleted?, mergedInto?}
//   Row:  {kind:'row', homeId, address:'handle::<serverId>', label,
//          workspaceId, order, updatedAt, deleted?}   identity (homeId, address)
// Last writer wins per item on (updatedAt, deviceId as a string); on a full
// tie a tombstone wins. `order` is a fractional-index key
// (`lib/fractional-index.ts`).
//
// State (`SyncState`, persisted beside the Homes):
//   known   — the last version of every item this device knows (server items
//             plus its own unsent edits), tombstones included.
//   outbox  — unsent edits by item key (latest wins). Offline edits queue
//             here and replay on reconnect, with their original stamps.
//   cursor  — the server cursor this device has caught up to.
//   redirects — merged-away Home id → the Home it went into.
//
// What syncs: Home names and order, rows and row order. What stays local:
// the selected Home, expanded state, and label / workspaceId repairs
// (`repairRows`), which would otherwise flip-flop between devices. A row's
// label travels when the row is created; an address change (a handle
// rename) is a delete plus an add.

import type { Home, HomeRow } from '@/stores/homes'
import { assignOrders, compareOrder, orderBetween } from '@/lib/fractional-index'

export interface WireHome {
  kind: 'home'
  id: string
  name: string | null
  order: string
  updatedAt: number
  deviceId?: string
  deleted?: boolean
  mergedInto?: string
}

export interface WireRow {
  kind: 'row'
  homeId: string
  address: string
  label: string
  workspaceId: string | null
  order: string
  updatedAt: number
  deviceId?: string
  deleted?: boolean
}

export type WireItem = WireHome | WireRow

export interface SyncState {
  version: 1
  deviceId: string
  /** The control epoch (`k2.homes.syncControl.v1`) this state last acted
   *  on. "Sync again" bumps it: the runner re-registers and re-merges. */
  epoch: number
  /** The first-sync union ran against `serverId`. */
  merged: boolean
  /** The home server's id when the union ran. */
  serverId: string | null
  cursor: number | null
  known: Record<string, WireItem>
  outbox: Record<string, WireItem>
  redirects: Record<string, string>
  /** The server removed this device (410): no sync until "Sync again". */
  removed: boolean
  lastSyncAt: number | null
}

/** A pull answer (`GET /cli/me/homes`). */
export interface PullAnswer {
  cursor: number
  full: boolean
  changes: WireItem[]
  redirects: Record<string, string>
  serverId: string | null
  servers?: Record<string, string>
  device?: { deviceId: string; syncing: boolean }
}

/** Address conversion at the sync edge (`server-ids.ts`). */
export interface AddressMap {
  /** Local `handle::hostKey` → wire `handle::<serverId>` (or the host key
   *  when the server's id isn't known yet). */
  toWire(address: string): string
  /** Wire → local. An unknown server id stays as is. */
  fromWire(address: string): string
}

export interface EngineCtx extends AddressMap {
  now: number
  deviceId: string
}

/** Tombstones this device keeps in `known` (the server keeps them ≥ 30 d). */
export const LOCAL_TOMBSTONE_KEEP_MS = 90 * 24 * 60 * 60 * 1000

export function emptySyncState(deviceId: string): SyncState {
  return {
    version: 1,
    deviceId,
    epoch: 0,
    merged: false,
    serverId: null,
    cursor: null,
    known: {},
    outbox: {},
    redirects: {},
    removed: false,
    lastSyncAt: null,
  }
}

export function itemKey(i: WireItem): string {
  return i.kind === 'home' ? `h|${i.id}` : `r|${i.homeId}|${i.address}`
}

/** Last writer wins: `updatedAt`, then `deviceId` (string compare), then a
 *  tombstone beats a live item on a full tie. */
export function beats(a: WireItem, b: WireItem): boolean {
  if (a.updatedAt !== b.updatedAt) return a.updatedAt > b.updatedAt
  const da = a.deviceId ?? ''
  const db = b.deviceId ?? ''
  if (da !== db) return da > db
  return !!a.deleted && !b.deleted
}

function isLive(i: WireItem | undefined): i is WireItem {
  return !!i && !i.deleted
}

/** A stamp newer than what this device last saw for the item (a local edit
 *  always beats the version it was made on, whatever the clocks say). */
function stamp(ctx: EngineCtx, prev: WireItem | undefined): number {
  return Math.max(ctx.now, (prev?.updatedAt ?? 0) + 1)
}

function nameKey(name: string | null): string {
  return (name ?? '').trim().replace(/\s+/g, ' ').toLocaleLowerCase()
}

/** Follow redirects (A → B → C), stopping on a loop. */
export function resolveRedirect(redirects: Readonly<Record<string, string>>, id: string): string {
  let at = id
  const seen = new Set<string>([at])
  while (redirects[at] !== undefined) {
    at = redirects[at]
    if (seen.has(at)) break
    seen.add(at)
  }
  return at
}

interface View {
  items: Map<string, WireItem>
}

/** The wire view of the local Homes: one item per Home and row, keyed,
 *  with order keys that keep every known key still in order. */
function localView(homes: readonly Home[], st: SyncState, ctx: AddressMap): View {
  const items = new Map<string, WireItem>()
  const homeOrders = assignOrders(homes.length, (i) => {
    const k = st.known[`h|${homes[i].id}`]
    return isLive(k) ? k.order : null
  })
  homes.forEach((h, hi) => {
    items.set(`h|${h.id}`, { kind: 'home', id: h.id, name: h.name, order: homeOrders[hi], updatedAt: 0 })
    const seen = new Set<string>()
    const rows: Array<{ row: HomeRow; address: string }> = []
    for (const r of h.rows) {
      const address = ctx.toWire(r.address)
      if (seen.has(address)) continue
      seen.add(address)
      rows.push({ row: r, address })
    }
    const rowOrders = assignOrders(rows.length, (i) => {
      const k = st.known[`r|${h.id}|${rows[i].address}`]
      return isLive(k) ? k.order : null
    })
    rows.forEach(({ row, address }, ri) => {
      items.set(`r|${h.id}|${address}`, {
        kind: 'row',
        homeId: h.id,
        address,
        label: row.label,
        workspaceId: row.workspaceId,
        order: rowOrders[ri],
        updatedAt: 0,
      })
    })
  })
  return { items }
}

/** Does a view item differ from the known one in what syncs? (Names and
 *  order; never labels or workspace ids.) */
function syncedFieldsDiffer(view: WireItem, known: WireItem): boolean {
  if (view.order !== known.order) return true
  if (view.kind === 'home' && known.kind === 'home') return view.name !== known.name
  return false
}

function queue(st: SyncState, item: WireItem): void {
  const k = itemKey(item)
  st.known[k] = item
  st.outbox[k] = item
}

function clone(st: SyncState): SyncState {
  return { ...st, known: { ...st.known }, outbox: { ...st.outbox }, redirects: { ...st.redirects } }
}

/**
 * Turn the local Homes into queued changes: new or reordered or renamed
 * items are upserted, items that left the local copy become tombstones.
 * Returns the new state and the changes it queued (empty when nothing that
 * syncs changed — label repairs never queue).
 */
export function diffLocal(homes: readonly Home[], prev: SyncState, ctx: EngineCtx): { state: SyncState; changes: WireItem[] } {
  const st = clone(prev)
  const view = localView(homes, st, ctx)
  const changes: WireItem[] = []
  for (const [k, v] of view.items) {
    const known = st.known[k]
    if (isLive(known) && !syncedFieldsDiffer(v, known)) continue
    const item: WireItem = { ...v, updatedAt: stamp(ctx, known), deviceId: ctx.deviceId }
    queue(st, item)
    changes.push(item)
  }
  for (const [k, known] of Object.entries(st.known)) {
    if (!isLive(known) || view.items.has(k)) continue
    const tomb: WireItem = { ...known, updatedAt: stamp(ctx, known), deviceId: ctx.deviceId, deleted: true }
    if (tomb.kind === 'home') delete tomb.mergedInto
    queue(st, tomb)
    changes.push(tomb)
  }
  return { state: st, changes }
}

/**
 * Apply a pull. A server item replaces the known one when it is newer (and
 * drops an unsent local edit it beats); an older one is ignored (the local
 * edit is newer and still queued). A full answer also forgets known live
 * items the server no longer has, unless they are queued here.
 */
export function applyPull(prev: SyncState, pull: PullAnswer, now: number): SyncState {
  const st = clone(prev)
  if (pull.full) {
    const present = new Set(pull.changes.map(itemKey))
    for (const [k, known] of Object.entries(st.known)) {
      if (present.has(k) || st.outbox[k]) continue
      if (isLive(known)) delete st.known[k]
    }
  }
  for (const c of pull.changes) {
    const k = itemKey(c)
    const known = st.known[k]
    if (known && !beats(c, known)) continue
    st.known[k] = c
    if (st.outbox[k]) delete st.outbox[k]
  }
  st.redirects = { ...st.redirects, ...pull.redirects }
  st.cursor = pull.cursor
  st.lastSyncAt = now
  // Forget old tombstones here too (not redirects: those are in `redirects`).
  for (const [k, known] of Object.entries(st.known)) {
    if (known.deleted && !st.outbox[k] && now - known.updatedAt > LOCAL_TOMBSTONE_KEEP_MS) delete st.known[k]
  }
  return st
}

/** A push answered: drop the sent items that are still the queued version. */
export function ackPush(prev: SyncState, sent: readonly WireItem[], rejected: ReadonlyArray<{ item: unknown }>): {
  state: SyncState
  needsFullPull: boolean
} {
  const st = clone(prev)
  for (const s of sent) {
    const k = itemKey(s)
    const q = st.outbox[k]
    if (q && q.updatedAt === s.updatedAt && q.deviceId === s.deviceId) delete st.outbox[k]
  }
  // A refused item (the last Home, a bad name) is forgotten locally; a full
  // pull then brings the server's truth back.
  let needsFullPull = false
  for (const r of rejected) {
    const item = r.item as Partial<WireItem> | null
    if (!item || (item.kind !== 'home' && item.kind !== 'row')) continue
    needsFullPull = true
    const k = item.kind === 'home' ? `h|${(item as WireHome).id}` : `r|${(item as WireRow).homeId}|${(item as WireRow).address}`
    delete st.known[k]
    delete st.outbox[k]
  }
  return { state: st, needsFullPull }
}

/**
 * The local Homes the known items describe: live Homes by order, their live
 * rows by order, addresses converted back to local host keys. The previous
 * local copy's labels and workspace ids win for rows it already had (they
 * are local repair hints). Null when there is no live Home (keep the local
 * copy as it is).
 */
export function buildLocal(st: SyncState, previous: readonly Home[], ctx: AddressMap): Home[] | null {
  const homes = Object.values(st.known).filter((i): i is WireHome => i.kind === 'home' && !i.deleted)
  if (homes.length === 0) return null
  homes.sort((a, b) => compareOrder(a.order, b.order) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  const hints = new Map<string, HomeRow>()
  for (const h of previous) for (const r of h.rows) if (!hints.has(r.address)) hints.set(r.address, r)
  const rowsByHome = new Map<string, WireRow[]>()
  for (const i of Object.values(st.known)) {
    if (i.kind !== 'row' || i.deleted) continue
    const list = rowsByHome.get(i.homeId) ?? []
    list.push(i)
    rowsByHome.set(i.homeId, list)
  }
  return homes.map((h) => {
    const wire = (rowsByHome.get(h.id) ?? []).sort(
      (a, b) => compareOrder(a.order, b.order) || (a.address < b.address ? -1 : a.address > b.address ? 1 : 0),
    )
    const seen = new Set<string>()
    const rows: HomeRow[] = []
    for (const w of wire) {
      const address = ctx.fromWire(w.address)
      if (seen.has(address)) continue
      seen.add(address)
      const hint = hints.get(address)
      rows.push({
        address,
        workspaceId: hint ? hint.workspaceId : w.workspaceId,
        label: hint ? hint.label : w.label,
      })
    }
    return { id: h.id, name: h.name ?? 'Home', rows }
  })
}

/**
 * The first sync with a server (or a re-sync after "Stop syncing" or a
 * removal): combine both lists, delete nothing. Homes match by name
 * (trimmed, case-insensitive); the server's Home id survives and the local
 * one becomes a tombstone `mergedInto` it (a redirect). Rows union by
 * address within a Home. Local-only Homes and rows are added after the
 * server's.
 */
export function firstSync(local: readonly Home[], pull: PullAnswer, prev: SyncState, ctx: EngineCtx): {
  state: SyncState
  changes: WireItem[]
} {
  const st: SyncState = {
    ...prev,
    known: {},
    outbox: {},
    redirects: { ...prev.redirects, ...pull.redirects },
    cursor: pull.cursor,
    merged: true,
    serverId: pull.serverId,
    removed: false,
    lastSyncAt: ctx.now,
  }
  for (const c of pull.changes) if (!c.deleted) st.known[itemKey(c)] = c
  const changes: WireItem[] = []
  const add = (item: WireItem): void => {
    queue(st, item)
    changes.push(item)
  }
  const serverHomes = Object.values(st.known)
    .filter((i): i is WireHome => i.kind === 'home')
    .sort((a, b) => compareOrder(a.order, b.order))
  const byName = new Map<string, string>()
  for (const h of serverHomes) if (!byName.has(nameKey(h.name))) byName.set(nameKey(h.name), h.id)
  let lastHomeOrder: string | null = serverHomes.length ? serverHomes[serverHomes.length - 1].order : null
  const lastRowOrder = (homeId: string): string | null => {
    let last: string | null = null
    for (const i of Object.values(st.known)) {
      if (i.kind === 'row' && i.homeId === homeId && !i.deleted && (last === null || i.order > last)) last = i.order
    }
    return last
  }
  for (const h of local) {
    let target: string
    if (isLive(st.known[`h|${h.id}`])) {
      target = h.id
    } else if (byName.has(nameKey(h.name))) {
      target = byName.get(nameKey(h.name)) as string
      st.redirects[h.id] = target
      add({ kind: 'home', id: h.id, name: h.name, order: '', updatedAt: ctx.now, deviceId: ctx.deviceId, deleted: true, mergedInto: target })
    } else {
      lastHomeOrder = orderBetween(lastHomeOrder, null)
      target = h.id
      add({ kind: 'home', id: h.id, name: h.name, order: lastHomeOrder, updatedAt: ctx.now, deviceId: ctx.deviceId })
      byName.set(nameKey(h.name), h.id)
    }
    for (const r of h.rows) {
      const address = ctx.toWire(r.address)
      if (isLive(st.known[`r|${target}|${address}`])) continue
      add({
        kind: 'row',
        homeId: target,
        address,
        label: r.label,
        workspaceId: r.workspaceId,
        order: orderBetween(lastRowOrder(target), null),
        updatedAt: ctx.now,
        deviceId: ctx.deviceId,
      })
    }
  }
  return { state: st, changes }
}

/** Parse a stored `k2.homes.sync.v1`; null when missing or not ours. */
export function parseSyncState(raw: string | null): SyncState | null {
  if (!raw) return null
  try {
    const v = JSON.parse(raw) as Partial<SyncState>
    if (v?.version !== 1 || typeof v.deviceId !== 'string' || !v.deviceId) return null
    return {
      version: 1,
      deviceId: v.deviceId,
      epoch: typeof v.epoch === 'number' ? v.epoch : 0,
      merged: v.merged === true,
      serverId: typeof v.serverId === 'string' ? v.serverId : null,
      cursor: typeof v.cursor === 'number' ? v.cursor : null,
      known: v.known && typeof v.known === 'object' ? v.known : {},
      outbox: v.outbox && typeof v.outbox === 'object' ? v.outbox : {},
      redirects: v.redirects && typeof v.redirects === 'object' ? v.redirects : {},
      removed: v.removed === true,
      lastSyncAt: typeof v.lastSyncAt === 'number' ? v.lastSyncAt : null,
    }
  } catch {
    return null
  }
}

/** Validate one item from a server answer (drops anything malformed). */
export function parseWireItem(v: unknown): WireItem | null {
  if (!v || typeof v !== 'object') return null
  const o = v as Record<string, unknown>
  if (typeof o.updatedAt !== 'number' || typeof o.order !== 'string') return null
  const deviceId = typeof o.deviceId === 'string' ? o.deviceId : undefined
  const deleted = o.deleted === true ? true : undefined
  if (o.kind === 'home' && typeof o.id === 'string' && o.id) {
    const item: WireHome = {
      kind: 'home',
      id: o.id,
      name: typeof o.name === 'string' ? o.name : null,
      order: o.order,
      updatedAt: o.updatedAt,
      deviceId,
    }
    if (deleted) item.deleted = true
    if (typeof o.mergedInto === 'string') item.mergedInto = o.mergedInto
    return item
  }
  if (o.kind === 'row' && typeof o.homeId === 'string' && typeof o.address === 'string' && o.address.includes('::')) {
    const item: WireRow = {
      kind: 'row',
      homeId: o.homeId,
      address: o.address,
      label: typeof o.label === 'string' ? o.label : '',
      workspaceId: typeof o.workspaceId === 'string' ? o.workspaceId : null,
      order: o.order,
      updatedAt: o.updatedAt,
      deviceId,
    }
    if (deleted) item.deleted = true
    return item
  }
  return null
}

/** Parse a `GET /cli/me/homes` answer. Throws on a body that isn't one. */
export function parsePullAnswer(v: unknown): PullAnswer {
  if (!v || typeof v !== 'object') throw new Error('homes sync: the pull answer is not an object')
  const o = v as Record<string, unknown>
  if (typeof o.cursor !== 'number' || !Array.isArray(o.changes)) throw new Error('homes sync: the pull answer has no cursor or changes')
  const redirects: Record<string, string> = {}
  if (o.redirects && typeof o.redirects === 'object') {
    for (const [k, val] of Object.entries(o.redirects as Record<string, unknown>)) if (typeof val === 'string') redirects[k] = val
  }
  const servers: Record<string, string> = {}
  if (o.servers && typeof o.servers === 'object') {
    for (const [k, val] of Object.entries(o.servers as Record<string, unknown>)) if (typeof val === 'string') servers[k] = val
  }
  const device = o.device as { deviceId?: unknown; syncing?: unknown } | undefined
  return {
    cursor: o.cursor,
    full: o.full === true,
    changes: o.changes.map(parseWireItem).filter((i): i is WireItem => i !== null),
    redirects,
    serverId: typeof o.serverId === 'string' ? o.serverId : null,
    servers,
    device:
      device && typeof device.deviceId === 'string' ? { deviceId: device.deviceId, syncing: device.syncing === true } : undefined,
  }
}
