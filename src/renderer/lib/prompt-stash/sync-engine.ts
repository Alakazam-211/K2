// stash-sync-v1 (0.46.7) — the pure half of prompt stash sync: the sync
// state this device keeps, merging a pull into the local records, and
// acknowledging a push. No storage, no network (`sync-runner.ts` does
// those). Contract: `.k2/prds/contract-stash-sync-v1.md`.
//
// Local-first, like Homes: IndexedDB stays the working copy. A local save
// or delete marks the record dirty; the runner pushes dirty records, then
// pulls and merges by the shared rule (`stashBeats`: newest `updated_at`,
// then the larger `updated_by`, then the tombstone). Attachment bytes
// never leave the device.

import { coerceStashRecord, stashBeats, type StashRecord } from './record'
import type { StashWrite } from './idb'

export const STASH_SYNC_STATE_KEY = 'k2.promptStash.sync.v1'
export const STASH_SYNC_FEATURE = 'stash-sync-v1'
/** The server takes at most this many records per push. */
export const STASH_PUSH_MAX = 200
/** Keep a push body under the server's 4 MiB. */
export const STASH_PUSH_MAX_BYTES = 3 * 1024 * 1024
/** The rejection that means "the person's stash is full on the server". */
export const CAP_REACHED = 'cap_reached'

export interface StashSyncState {
  version: 1
  /** The server cursor from the last pull (null: never pulled). */
  cursor: number | null
  /** Records to push: id → the `updated_at` that made it dirty. */
  dirty: Record<string, number>
  /** Records the server refused: id → reason. `cap_reached` ones are sent
   *  again after the next local change; the others only once edited. A
   *  held record is never dropped by a full pull. */
  held: Record<string, string>
  /** The first sync ran (every local record was queued once). */
  started: boolean
}

export function emptyStashSyncState(): StashSyncState {
  return { version: 1, cursor: null, dirty: {}, held: {}, started: false }
}

export function parseStashSyncState(raw: string | null): StashSyncState | null {
  if (!raw) return null
  try {
    const v = JSON.parse(raw) as Partial<StashSyncState>
    if (v?.version !== 1) return null
    const nums = (o: unknown): Record<string, number> =>
      o && typeof o === 'object'
        ? Object.fromEntries(Object.entries(o as Record<string, unknown>).filter((e): e is [string, number] => typeof e[1] === 'number'))
        : {}
    const strs = (o: unknown): Record<string, string> =>
      o && typeof o === 'object'
        ? Object.fromEntries(Object.entries(o as Record<string, unknown>).filter((e): e is [string, string] => typeof e[1] === 'string'))
        : {}
    return {
      version: 1,
      cursor: typeof v.cursor === 'number' ? v.cursor : null,
      dirty: nums(v.dirty),
      held: strs(v.held),
      started: v.started === true,
    }
  } catch {
    return null
  }
}

/** Mark `records` dirty (a local save or delete). A local change also
 *  frees the records held at the cap: they are tried again. */
export function markDirty(st: StashSyncState, records: ReadonlyArray<Pick<StashRecord, 'id' | 'updated_at'>>): StashSyncState {
  const dirty = { ...st.dirty }
  const held = { ...st.held }
  for (const [id, reason] of Object.entries(st.held)) {
    if (reason === CAP_REACHED) {
      dirty[id] = dirty[id] ?? 0
      delete held[id]
    }
  }
  for (const r of records) {
    dirty[r.id] = r.updated_at
    delete held[r.id]
  }
  return { ...st, dirty, held }
}

/** The first sync: every local record goes up once. */
export function startSync(st: StashSyncState, local: readonly StashRecord[]): StashSyncState {
  if (st.started) return st
  return { ...markDirty(st, local), started: true }
}

/** Forget dirty ids (of `candidates`, default all) that have no local
 *  record any more. */
export function pruneDirty(st: StashSyncState, local: readonly StashRecord[], candidates: readonly string[] = Object.keys(st.dirty)): StashSyncState {
  const ids = new Set(local.map((r) => r.id))
  const gone = candidates.filter((id) => id in st.dirty && !ids.has(id))
  if (gone.length === 0) return st
  const dirty = { ...st.dirty }
  for (const id of gone) delete dirty[id]
  return { ...st, dirty }
}

/** The dirty records to push now (their current local version), in
 *  batches the server accepts. */
export function pushBatches(st: StashSyncState, local: readonly StashRecord[]): StashRecord[][] {
  const recs = local.filter((r) => r.id in st.dirty)
  const out: StashRecord[][] = []
  let cur: StashRecord[] = []
  let bytes = 0
  for (const r of recs) {
    const size = JSON.stringify(r).length
    if (cur.length > 0 && (cur.length >= STASH_PUSH_MAX || bytes + size > STASH_PUSH_MAX_BYTES)) {
      out.push(cur)
      cur = []
      bytes = 0
    }
    cur.push(r)
    bytes += size
  }
  if (cur.length) out.push(cur)
  return out
}

export interface PushAnswer {
  rejected: Array<{ id: string | null; reason: string }>
}

export function parsePushAnswer(raw: unknown): PushAnswer {
  const o = raw as { rejected?: unknown } | null
  if (!o || typeof o !== 'object' || !Array.isArray(o.rejected)) throw new Error('stash sync: the push answer has no rejected list')
  return {
    rejected: o.rejected.map((x) => {
      const r = x as { item?: { id?: unknown } | null; reason?: unknown } | null
      return { id: typeof r?.item?.id === 'string' ? r.item.id : null, reason: typeof r?.reason === 'string' ? r.reason : 'rejected' }
    }),
  }
}

/** After a push: what the server took is no longer dirty (unless it
 *  changed again meanwhile); what it refused is held with its reason. */
export function ackPush(st: StashSyncState, sent: readonly StashRecord[], answer: PushAnswer): StashSyncState {
  const dirty = { ...st.dirty }
  const held = { ...st.held }
  const refused = new Map(answer.rejected.filter((r) => r.id !== null).map((r) => [r.id as string, r.reason]))
  for (const r of sent) {
    if (dirty[r.id] !== undefined && dirty[r.id] <= r.updated_at) delete dirty[r.id]
    const reason = refused.get(r.id)
    if (reason) held[r.id] = reason
    else delete held[r.id]
  }
  return { ...st, dirty, held }
}

export interface PullAnswer {
  cursor: number
  full: boolean
  /** Records this client reads (`v: 1`); others were skipped. */
  changes: StashRecord[]
}

export function parsePullAnswer(raw: unknown): PullAnswer {
  const o = raw as { cursor?: unknown; full?: unknown; changes?: unknown } | null
  if (!o || typeof o !== 'object' || typeof o.cursor !== 'number' || !Array.isArray(o.changes)) {
    throw new Error('stash sync: the pull answer is not a stash pull')
  }
  return {
    cursor: o.cursor,
    full: o.full === true,
    // Unknown `v`: never shown, never written (field rules).
    changes: o.changes.flatMap((c) => {
      const r = coerceStashRecord(c)
      return r ? [r] : []
    }),
  }
}

/** A full answer whose cursor is behind ours: the server lost its stash (a
 *  reset or restored database), not the person. Nothing local is
 *  forgotten; the runner pushes everything again. */
export function isServerReset(st: StashSyncState, pull: PullAnswer): boolean {
  return pull.full && st.cursor !== null && pull.cursor < st.cursor
}

/** Merge a pull into the local records: one IndexedDB write. A remote
 *  record lands when it wins (`stashBeats`); a full pull also forgets local
 *  records the server no longer has (their tombstones were collected),
 *  unless they wait to be pushed or are held. Bytes of attachments that
 *  are gone from the winning version are dropped. */
export function mergePull(local: readonly StashRecord[], pull: PullAnswer, st: StashSyncState): StashWrite & { changed: boolean } {
  const byId = new Map(local.map((r) => [r.id, r]))
  const put: StashRecord[] = []
  const deleteRecords: string[] = []
  const deleteBlobs: string[] = []
  const dropBlobs = (from: StashRecord, keep: StashRecord | null): void => {
    const kept = new Set((keep?.attachments ?? []).map((a) => a.id))
    for (const a of from.attachments) if (!kept.has(a.id)) deleteBlobs.push(a.id)
  }
  for (const r of pull.changes) {
    const l = byId.get(r.id)
    if (l && !stashBeats(r, l)) continue
    put.push(r)
    if (l) dropBlobs(l, r)
  }
  if (pull.full && !isServerReset(st, pull)) {
    const remote = new Set(pull.changes.map((r) => r.id))
    for (const l of local) {
      if (remote.has(l.id) || l.id in st.dirty || l.id in st.held) continue
      deleteRecords.push(l.id)
      dropBlobs(l, null)
    }
  }
  return { put, deleteRecords, deleteBlobs, changed: put.length + deleteRecords.length > 0 }
}
