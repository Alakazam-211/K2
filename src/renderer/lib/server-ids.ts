// homes-sync-v1 (0.46.1) — which server is which, by `serverId`.
//
// A Home row on this computer names its agent `handle::<hostKey>`: `local`
// for this computer's daemon, a saved server's hostname otherwise. That is
// device-relative: the laptop's `atlas::local` is the phone's
// `atlas::rosson.k2.dev`. Synced rows name the server by its `serverId`
// instead (`/boot-status` `serverId`: the first 16 hex of SHA-256 over the
// box's federation identity key, stable across restarts), so every device
// means the same agent.
//
// This module remembers hostKey → serverId (localStorage `k2.serverIds.v1`,
// shared by every window) from whoever reads a server's `/boot-status`
// (the Home pool, the ConnectionGate, the sync runner), and from the home
// server's own `servers` map (its tunnel host and pinned federation peers).
// A `/boot-status` answer is the truth for its host; the `servers` map only
// fills hosts nothing has answered for.
//
// `toWireAddress` / `fromWireAddress` are the sync edge. Local storage and
// every renderer call site keep `handle::hostKey`; only the sync layer
// sees `handle::<serverId>`. A host whose id isn't known yet goes on the
// wire as its host key (the server accepts any `handle::host`) and moves to
// its id the first time it answers: the hostKey → serverId migration.

import { LOCAL_HOME_HOST, canonicalHostKey } from '@/lib/host-key'
import { parseHomeAddress } from '@/lib/home-address'

export const SERVER_IDS_STORAGE_KEY = 'k2.serverIds.v1'

const SERVER_ID_RE = /^[0-9a-f]{16}$/

export function isServerId(s: string): boolean {
  return SERVER_ID_RE.test(s)
}

interface Stored {
  version: 1
  /** hostKey → serverId, from that host's own `/boot-status`. */
  byHost: Record<string, string>
  /** hostKey → serverId, vouched for by a home server's `servers` map. */
  hinted: Record<string, string>
  /** An id a host used to answer with (the box was cloned or migrated and
   *  has a new identity key) → that host. Rows still naming the old id
   *  resolve to the host, then move to its new id (research P12). */
  retired: Record<string, string>
}

type Kv = Pick<Storage, 'getItem' | 'setItem'>

function storage(): Kv | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

let memo: Stored | null = null
const listeners = new Set<() => void>()

function load(): Stored {
  if (memo) return memo
  const empty: Stored = { version: 1, byHost: {}, hinted: {}, retired: {} }
  try {
    const raw = storage()?.getItem(SERVER_IDS_STORAGE_KEY) ?? null
    if (raw) {
      const v = JSON.parse(raw) as Partial<Stored>
      if (v?.version === 1) {
        memo = {
          version: 1,
          byHost: v.byHost && typeof v.byHost === 'object' ? { ...v.byHost } : {},
          hinted: v.hinted && typeof v.hinted === 'object' ? { ...v.hinted } : {},
          retired: v.retired && typeof v.retired === 'object' ? { ...v.retired } : {},
        }
        return memo
      }
    }
  } catch (err) {
    console.warn('[server-ids] storage read failed:', err)
  }
  memo = empty
  return memo
}

function save(next: Stored): void {
  memo = next
  try {
    storage()?.setItem(SERVER_IDS_STORAGE_KEY, JSON.stringify(next))
  } catch (err) {
    console.warn('[server-ids] storage write failed:', err)
  }
  for (const fn of listeners) fn()
}

function keyOf(hostKey: string): string {
  return hostKey === LOCAL_HOME_HOST ? LOCAL_HOME_HOST : canonicalHostKey(hostKey)
}

/** Another window wrote the map (`storage` event): re-read on next use. */
export function invalidateServerIds(): void {
  memo = null
  for (const fn of listeners) fn()
}

/** Called when the map changes (a server's id learned). */
export function onServerIdsChanged(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

/** Note what a server's `/boot-status` said its `serverId` is. Ignored
 *  when the body had none (an older daemon) or it is malformed. */
export function noteServerId(hostKey: string, serverId: unknown): void {
  if (typeof serverId !== 'string' || !isServerId(serverId)) return
  const k = keyOf(hostKey)
  const cur = load()
  if (cur.byHost[k] === serverId) return
  const old = cur.byHost[k]
  const retired = { ...cur.retired }
  if (old) retired[old] = k
  delete retired[serverId]
  save({ ...cur, byHost: { ...cur.byHost, [k]: serverId }, retired })
}

/** Note a home server's `servers` map (host key → serverId). Never
 *  overrides a host's own `/boot-status` answer. */
export function noteServerMap(map: Readonly<Record<string, string>> | undefined): void {
  if (!map) return
  const cur = load()
  let changed = false
  const hinted = { ...cur.hinted }
  for (const [host, id] of Object.entries(map)) {
    if (!isServerId(id)) continue
    const k = keyOf(host)
    if (cur.byHost[k] || hinted[k] === id) continue
    hinted[k] = id
    changed = true
  }
  if (changed) save({ ...cur, hinted })
}

/** The serverId of a host key, or null when nothing has said. */
export function serverIdForHost(hostKey: string): string | null {
  const k = keyOf(hostKey)
  const cur = load()
  return cur.byHost[k] ?? cur.hinted[k] ?? null
}

/** The host key this device knows a serverId by: `local` when it is this
 *  computer's daemon, else a saved server. `isSaved` breaks a tie when
 *  two host keys reach the same box (a LAN address and its tunnel host). */
export function hostKeyForServerId(serverId: string, isSaved: (hostKey: string) => boolean = () => true): string | null {
  const cur = load()
  const local = cur.byHost[LOCAL_HOME_HOST]
  if (local === serverId) return LOCAL_HOME_HOST
  const candidates = [
    ...Object.entries(cur.byHost).filter(([, id]) => id === serverId).map(([h]) => h),
    ...Object.entries(cur.hinted).filter(([h, id]) => id === serverId && !cur.byHost[h]).map(([h]) => h),
  ]
  const found = candidates.find(isSaved)
  if (found) return found
  const was = cur.retired[serverId]
  return was && (was === LOCAL_HOME_HOST || isSaved(was)) ? was : null
}

/** Local `handle::hostKey` → wire `handle::<serverId>` (or unchanged when
 *  the host's id isn't known). */
export function toWireAddress(address: string): string {
  const p = parseHomeAddress(address)
  if (!p) return address
  if (isServerId(p.host)) return `${p.handle}::${p.host}`
  const id = serverIdForHost(p.host)
  return id ? `${p.handle}::${id}` : `${p.handle}::${p.host}`
}

/** Wire → local: a known serverId becomes this device's host key for it;
 *  an unknown id (a server this device hasn't saved) stays as is. */
export function fromWireAddress(address: string, isSaved?: (hostKey: string) => boolean): string {
  const p = parseHomeAddress(address)
  if (!p || !isServerId(p.host)) return p ? `${p.handle}::${p.host}` : address
  const host = hostKeyForServerId(p.host, isSaved)
  return host ? `${p.handle}::${host}` : `${p.handle}::${p.host}`
}

/** Tests: forget the in-memory copy. */
export function resetServerIdsForTests(): void {
  memo = null
  listeners.clear()
}
