// stash-sync-v1 (0.46.7) — the desktop's prompt stash sync runner.
//
// Mirrors the Homes runner (`lib/homes-sync/runner.ts`): it syncs with this
// computer's own daemon, one window runs it (Web Lock `k2.stash.sync`), and
// the stash (IndexedDB) stays the working copy, so an offline daemon only
// pauses sync. The device is the Homes device (one registry per person):
// it follows Homes "Stop syncing" / "Sync again" (`k2.homes.syncControl.v1`)
// and a removed device (410) stops until Homes syncs again.
//
// Step: (first time: queue every local record) → push the dirty records →
// pull since the cursor → merge into IndexedDB. Runs on start, on every
// local change (debounced), on `stash_changed` with another cursor, on
// focus / online, and on a 60 s tick while visible. Network errors back off.

import { create } from 'zustand'
import { readControl, type KeyValue } from '@/lib/homes-sync/runner'
import type { StashRecord } from './record'
import type { StashWrite } from './idb'
import {
  ackPush,
  emptyStashSyncState,
  isServerReset,
  markDirty,
  mergePull,
  parsePullAnswer,
  parsePushAnswer,
  parseStashSyncState,
  pruneDirty,
  pushBatches,
  startSync,
  CAP_REACHED,
  STASH_SYNC_STATE_KEY,
  type StashSyncState,
} from './sync-engine'

export type StashSyncPhase = 'off' | 'unsupported' | 'syncing' | 'synced' | 'offline' | 'removed' | 'error'

export interface StashSyncStatus {
  phase: StashSyncPhase
  /** Records refused because the stash is full on the server. */
  heldAtCap: number
  error: string | null
}

export const useStashSyncStatus = create<StashSyncStatus>(() => ({ phase: 'off', heldAtCap: 0, error: null }))

export interface StashSyncTransport {
  /** Does the home server report `stash-sync-v1`? */
  supported(): boolean
  pull(deviceId: string, since: number | null): Promise<unknown>
  push(deviceId: string, changes: StashRecord[]): Promise<unknown>
  register(deviceId: string, name: string, platform: string): Promise<unknown>
}

/** The local stash, under the store's write queue. */
export interface StashLocal {
  /** Run `fn` on a fresh read of every record, write what it returns, in
   *  the store's serial queue (so a save never races a merge). */
  apply(fn: (all: StashRecord[]) => StashWrite | null): Promise<void>
  readAll(): Promise<StashRecord[]>
}

export interface StashRunnerDeps {
  transport: StashSyncTransport
  local: StashLocal
  kv: KeyValue | null
  deviceId: () => string | null
  device: { name: string; platform: string }
  onStatus?: (s: StashSyncStatus) => void
  debounceMs?: number
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (t: unknown) => void
}

function statusOf(err: unknown): number | null {
  const s = (err as { status?: unknown } | null)?.status
  return typeof s === 'number' ? s : null
}

const BACKOFF_MS = [2_000, 5_000, 15_000, 30_000, 60_000]

/** Read / write the sync state (any window: the store marks dirty). */
export function loadStashSyncState(kv: KeyValue | null): StashSyncState {
  try {
    return parseStashSyncState(kv?.getItem(STASH_SYNC_STATE_KEY) ?? null) ?? emptyStashSyncState()
  } catch {
    return emptyStashSyncState()
  }
}

export function saveStashSyncState(kv: KeyValue | null, st: StashSyncState): void {
  try {
    kv?.setItem(STASH_SYNC_STATE_KEY, JSON.stringify(st))
  } catch (err) {
    console.warn('[stash-sync] state write failed:', err)
  }
}

/** A local save or delete: queue it for the next push. */
export function noteLocalStashChange(kv: KeyValue | null, records: ReadonlyArray<Pick<StashRecord, 'id' | 'updated_at'>>): void {
  saveStashSyncState(kv, markDirty(loadStashSyncState(kv), records))
}

export class StashSyncRunner {
  private readonly d: StashRunnerDeps
  private running = false
  private inflight: Promise<void> | null = null
  private again = false
  private removedEpoch: number | null = null
  private debounce: unknown = null
  private retry: unknown = null
  private failures = 0
  private status: StashSyncStatus = { phase: 'off', heldAtCap: 0, error: null }

  constructor(deps: StashRunnerDeps) {
    this.d = deps
  }

  start(): void {
    if (this.running) return
    this.running = true
    void this.syncNow()
  }

  stop(): void {
    this.running = false
    this.clear('debounce')
    this.clear('retry')
  }

  getStatus(): StashSyncStatus {
    return this.status
  }

  private setStatus(patch: Partial<StashSyncStatus>): void {
    const st = loadStashSyncState(this.d.kv)
    const heldAtCap = Object.values(st.held).filter((r) => r === CAP_REACHED).length
    this.status = { ...this.status, heldAtCap, ...patch }
    this.d.onStatus?.(this.status)
  }

  private clear(which: 'debounce' | 'retry'): void {
    const t = this[which]
    if (t !== null) (this.d.clearTimer ?? clearTimeout)(t as ReturnType<typeof setTimeout>)
    this[which] = null
  }

  private later(which: 'debounce' | 'retry', fn: () => void, ms: number): void {
    this.clear(which)
    this[which] = (this.d.setTimer ?? setTimeout)(() => {
      this[which] = null
      fn()
    }, ms)
  }

  /** A local change (this or another window): push soon. */
  localChanged(): void {
    if (!this.running) return
    this.later('debounce', () => void this.syncNow(), this.d.debounceMs ?? 300)
  }

  /** A `stash_changed` frame: pull when the server moved. */
  onRemoteCursor(cursor: number): void {
    if (loadStashSyncState(this.d.kv).cursor === cursor) return
    void this.syncNow()
  }

  /** A `homes_device_removed` frame: when it is this device, stop. */
  onDeviceRemoved(deviceId: string): void {
    if (deviceId !== this.d.deviceId()) return
    this.removedEpoch = readControl(this.d.kv).epoch
    this.setStatus({ phase: 'removed', error: null })
  }

  poke(): void {
    void this.syncNow()
  }

  syncNow(): Promise<void> {
    if (!this.running) return Promise.resolve()
    if (this.inflight) {
      this.again = true
      return this.inflight
    }
    this.clear('debounce')
    this.inflight = (async () => {
      try {
        // A few rounds at most: edits made during a round, a server reset.
        let rounds = 0
        do {
          this.again = false
          await this.step()
        } while (this.again && this.running && ++rounds < 4)
      } finally {
        this.inflight = null
      }
    })()
    return this.inflight
  }

  private async push(deviceId: string, batch: StashRecord[]): Promise<unknown> {
    try {
      return await this.d.transport.push(deviceId, batch)
    } catch (err) {
      if (statusOf(err) !== 409) throw err
      // Not in the device registry yet (Homes hasn't registered, or a
      // wiped server): register and try once more.
      await this.d.transport.register(deviceId, this.d.device.name, this.d.device.platform)
      return this.d.transport.push(deviceId, batch)
    }
  }

  private async step(): Promise<void> {
    const control = readControl(this.d.kv)
    if (!control.enabled) {
      this.setStatus({ phase: 'off', error: null })
      return
    }
    if (this.removedEpoch !== null) {
      // "Sync again" in Homes bumps the epoch and registers the device.
      if (this.removedEpoch === control.epoch) {
        this.setStatus({ phase: 'removed', error: null })
        return
      }
      this.removedEpoch = null
    }
    if (!this.d.transport.supported()) {
      this.setStatus({ phase: 'unsupported', error: null })
      return
    }
    const deviceId = this.d.deviceId()
    if (!deviceId) {
      this.setStatus({ phase: 'error', error: 'This device has no id (storage is unavailable)' })
      return
    }
    this.setStatus({ phase: 'syncing', error: null })
    try {
      // The state first, then the records: a save writes its record before
      // marking it dirty, so every dirty id read here has its record below.
      const before = loadStashSyncState(this.d.kv)
      const local = await this.d.local.readAll()
      let st = loadStashSyncState(this.d.kv)
      if (!st.started) st = startSync(st, local)
      // A dirty id with no local record left (an expired tombstone) has
      // nothing to send. Only ids marked before the read: a mark made
      // during it has its record, read next round.
      st = pruneDirty(st, local, Object.keys(before.dirty))
      saveStashSyncState(this.d.kv, st)
      // Push what changed here.
      for (const batch of pushBatches(st, local)) {
        const answer = parsePushAnswer(await this.push(deviceId, batch))
        saveStashSyncState(this.d.kv, ackPush(loadStashSyncState(this.d.kv), batch, answer))
      }
      // Pull and merge (in the store's queue, on a fresh read).
      st = loadStashSyncState(this.d.kv)
      const pull = parsePullAnswer(await this.d.transport.pull(deviceId, st.cursor))
      await this.d.local.apply((all) => {
        const cur = loadStashSyncState(this.d.kv)
        const w = mergePull(all, pull, cur)
        let next: StashSyncState = { ...cur, cursor: pull.cursor }
        if (isServerReset(cur, pull)) {
          // The server lost the stash: everything live here goes up again.
          next = markDirty(next, all.filter((r) => r.deleted_at === null))
          this.again = true
        }
        saveStashSyncState(this.d.kv, next)
        return w.changed ? w : null
      })
      // Changed while we were away: one more round.
      if (Object.keys(loadStashSyncState(this.d.kv).dirty).length > 0) this.again = true
      this.failures = 0
      this.setStatus({ phase: 'synced', error: null })
    } catch (err) {
      const status = statusOf(err)
      if (status === 410) {
        this.removedEpoch = control.epoch
        this.setStatus({ phase: 'removed', error: null })
        return
      }
      if (status === 403 || status === 404) {
        this.setStatus({ phase: 'unsupported', error: null })
        return
      }
      const delay = BACKOFF_MS[Math.min(this.failures, BACKOFF_MS.length - 1)]
      this.failures++
      this.setStatus({ phase: status === null ? 'offline' : 'error', error: err instanceof Error ? err.message : String(err) })
      if (this.running) this.later('retry', () => void this.syncNow(), delay)
    }
  }
}
