// homes-sync-v1 (0.46.1) — the desktop's Homes sync runner.
//
// The desktop syncs its Homes with its OWN daemon by default (Rosson,
// ticket 4f537560): `scopeForHost('local')`, owner token. Local-first: the
// Homes store (`stores/homes.ts`, localStorage `k2.homes.v1`) stays the
// working copy and renders whatever happens here; an offline or old daemon
// only pauses sync ("Sync paused · 3 changes waiting"), never the Home
// screen.
//
// One runner per app: the window holding the Web Lock `k2.homes.sync`
// runs it; other windows follow the Homes through the `storage` event, and
// the status through `k2.homes.syncStatus.v1`. Settings in any window
// writes only the control key `k2.homes.syncControl.v1` ({enabled, epoch})
// plus its own HTTP calls; the runner reads the control before every step.
//
// Loop (no timers in the daemon; this is client polling of a peer):
//   start → register this device → pull (full + first-sync union the first
//   time, else since the cursor) → push the outbox.
//   A local edit → diff → queue → push (debounced).
//   `homes_changed` with a newer cursor, window focus, `online`, and a
//   60 s tick while visible → pull. Network errors back off (2 s … 60 s).
//   410 `device_removed` → stop until "Sync again".

import { create } from 'zustand'
import type { StoreApi, UseBoundStore } from 'zustand'
import type { HomesState } from '@/stores/homes'
import { HOMES_SYNC_STORAGE_KEY } from '@/stores/homes'
import {
  ackPush,
  applyPull,
  buildLocal,
  diffLocal,
  emptySyncState,
  firstSync,
  parsePullAnswer,
  parseSyncState,
  type AddressMap,
  type PullAnswer,
  type SyncState,
  type WireItem,
} from './engine'

export const HOMES_SYNC_CONTROL_KEY = 'k2.homes.syncControl.v1'
export const HOMES_SYNC_STATUS_KEY = 'k2.homes.syncStatus.v1'
export const HOMES_SYNC_LOCK = 'k2.homes.sync'
export const HOMES_SYNC_FEATURE = 'homes-sync-v1'

export type HomesSyncPhase =
  /** Not started yet, or the person turned sync off. */
  | 'off'
  /** This computer's daemon has no `homes-sync-v1` (or no person). */
  | 'unsupported'
  | 'syncing'
  | 'synced'
  /** The home server can't be reached: edits wait in the outbox. */
  | 'offline'
  /** The server removed this device: "Sync again" re-registers. */
  | 'removed'
  | 'error'

export interface HomesSyncStatus {
  phase: HomesSyncPhase
  /** Changes waiting to reach the home server. */
  pending: number
  lastSyncAt: number | null
  /** The home server: always this computer's daemon on the desktop. */
  serverLabel: string
  serverId: string | null
  deviceId: string | null
  error: string | null
}

export interface HomesSyncControl {
  enabled: boolean
  epoch: number
}

export const INITIAL_STATUS: HomesSyncStatus = {
  phase: 'off',
  pending: 0,
  lastSyncAt: null,
  serverLabel: 'This computer',
  serverId: null,
  deviceId: null,
  error: null,
}

/** What the runner talks to (the real one is `localTransport()`). */
export interface HomesSyncTransport {
  /** Does the home server report `homes-sync-v1`? */
  supported(): boolean
  pull(deviceId: string, since: number | null): Promise<unknown>
  push(deviceId: string, changes: WireItem[]): Promise<unknown>
  register(deviceId: string, name: string, platform: string): Promise<unknown>
  remove(deviceId: string): Promise<unknown>
  listDevices(deviceId: string): Promise<unknown>
}

export interface KeyValue {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

export interface RunnerDeps {
  store: UseBoundStore<StoreApi<HomesState>>
  transport: HomesSyncTransport
  kv: KeyValue | null
  addresses: AddressMap
  /** Learn the home server's id (`local` → serverId) from a pull. */
  noteServerId: (serverId: string | null) => void
  noteServerMap?: (map: Record<string, string> | undefined) => void
  now: () => number
  newDeviceId: () => string
  device: { name: string; platform: string }
  /** Status sink (the zustand store + the shared status key). */
  onStatus: (s: HomesSyncStatus) => void
  /** Push debounce (ms). Tests pass 0. */
  debounceMs?: number
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (t: unknown) => void
}

/** An HTTP status from a thrown daemon error, or null (network etc.). */
function statusOf(err: unknown): number | null {
  const s = (err as { status?: unknown } | null)?.status
  return typeof s === 'number' ? s : null
}

const BACKOFF_MS = [2_000, 5_000, 15_000, 30_000, 60_000]

export function readControl(kv: KeyValue | null): HomesSyncControl {
  try {
    const raw = kv?.getItem(HOMES_SYNC_CONTROL_KEY) ?? null
    if (raw) {
      const v = JSON.parse(raw) as Partial<HomesSyncControl>
      return { enabled: v.enabled !== false, epoch: typeof v.epoch === 'number' ? v.epoch : 0 }
    }
  } catch {
    /* unreadable: the default */
  }
  return { enabled: true, epoch: 0 }
}

export function writeControl(kv: KeyValue | null, c: HomesSyncControl): void {
  try {
    kv?.setItem(HOMES_SYNC_CONTROL_KEY, JSON.stringify(c))
  } catch (err) {
    console.warn('[homes-sync] control write failed:', err)
  }
}

export class HomesSyncRunner {
  private readonly d: RunnerDeps
  private running = false
  private inflight: Promise<void> | null = null
  private again = false
  private applying = false
  private registeredEpoch: number | null = null
  private debounce: unknown = null
  private retry: unknown = null
  private failures = 0
  private unsub: (() => void) | null = null
  private status: HomesSyncStatus

  constructor(deps: RunnerDeps) {
    this.d = deps
    const st = this.load()
    this.status = { ...INITIAL_STATUS, deviceId: st.deviceId, serverId: st.serverId, lastSyncAt: st.lastSyncAt }
  }

  // ── State ──────────────────────────────────────────────────────────

  /** The persisted sync state (a fresh one with a new device id when
   *  missing or unreadable — the first run on this computer). */
  load(): SyncState {
    let raw: string | null = null
    try {
      raw = this.d.kv?.getItem(HOMES_SYNC_STORAGE_KEY) ?? null
    } catch {
      raw = null
    }
    const st = parseSyncState(raw)
    if (st) return st
    const fresh = emptySyncState(this.d.newDeviceId())
    this.save(fresh)
    return fresh
  }

  private save(st: SyncState): void {
    try {
      this.d.kv?.setItem(HOMES_SYNC_STORAGE_KEY, JSON.stringify(st))
    } catch (err) {
      console.warn('[homes-sync] state write failed:', err)
    }
  }

  private setStatus(patch: Partial<HomesSyncStatus>, st?: SyncState): void {
    const s = st ?? this.load()
    this.status = {
      ...this.status,
      pending: Object.keys(s.outbox).length,
      lastSyncAt: s.lastSyncAt,
      serverId: s.serverId,
      deviceId: s.deviceId,
      ...patch,
    }
    this.d.onStatus(this.status)
  }

  getStatus(): HomesSyncStatus {
    return this.status
  }

  // ── Lifecycle ──────────────────────────────────────────────────────

  start(): void {
    if (this.running) return
    this.running = true
    if (this.d.store.getState().storageOk) {
      // Local edits → queue → push. Our own applySynced is skipped.
      this.unsub = this.d.store.subscribe((s, prev) => {
        if (s.homes !== prev.homes && !this.applying) this.scheduleDiff()
      })
    }
    void this.syncNow()
  }

  stop(): void {
    this.running = false
    this.unsub?.()
    this.unsub = null
    this.clear('debounce')
    this.clear('retry')
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

  private scheduleDiff(): void {
    this.later('debounce', () => void this.syncNow(), this.d.debounceMs ?? 300)
  }

  /** A `homes_changed` frame: pull when the server moved past us. */
  onRemoteCursor(cursor: number): void {
    if (this.load().cursor === cursor) return
    void this.syncNow()
  }

  /** A `homes_device_removed` frame. */
  onDeviceRemoved(deviceId: string): void {
    const st = this.load()
    if (deviceId !== st.deviceId) return
    this.markRemoved(st)
  }

  private markRemoved(st: SyncState): void {
    const next = { ...st, removed: true }
    this.save(next)
    this.registeredEpoch = null
    this.setStatus({ phase: 'removed', error: null }, next)
  }

  // ── The sync step ─────────────────────────────────────────────────

  /** Run one sync step now (serialized: a call during a step runs one more
   *  step after it). Resolves when this round is done. */
  syncNow(): Promise<void> {
    if (!this.running) return Promise.resolve()
    if (this.inflight) {
      this.again = true
      return this.inflight
    }
    this.clear('debounce')
    this.inflight = (async () => {
      try {
        do {
          this.again = false
          await this.step()
        } while (this.again && this.running)
      } finally {
        this.inflight = null
      }
    })()
    return this.inflight
  }

  /** Queue whatever the local copy has that the state doesn't (sync). */
  private queueLocal(st: SyncState): SyncState {
    if (!st.merged || !this.d.store.getState().storageOk) return st
    const { state, changes } = diffLocal(this.d.store.getState().homes, st, {
      ...this.d.addresses,
      now: this.d.now(),
      deviceId: st.deviceId,
    })
    if (changes.length) this.save(state)
    return changes.length ? state : st
  }

  /** Write the known items back into the Homes store (sync, no await). */
  private applyLocal(st: SyncState): void {
    const s = this.d.store.getState()
    const homes = buildLocal(st, s.homes, this.d.addresses)
    const sameRedirects = JSON.stringify(s.redirects) === JSON.stringify(st.redirects)
    if (!homes) {
      if (!sameRedirects) s.setRedirects(st.redirects)
      return
    }
    if (JSON.stringify(homes) === JSON.stringify(s.homes) && sameRedirects) return
    this.applying = true
    try {
      s.applySynced(homes, st.redirects)
    } finally {
      this.applying = false
    }
  }

  private async step(): Promise<void> {
    const control = readControl(this.d.kv)
    let st = this.load()
    if (control.epoch !== st.epoch) {
      // "Sync again": re-register and run the union again.
      st = { ...st, epoch: control.epoch, removed: false, merged: false }
      this.save(st)
      this.registeredEpoch = null
    }
    if (!control.enabled) {
      this.setStatus({ phase: 'off', error: null }, st)
      return
    }
    if (st.removed) {
      this.setStatus({ phase: 'removed', error: null }, st)
      return
    }
    if (!this.d.transport.supported()) {
      this.setStatus({ phase: 'unsupported', error: null }, st)
      return
    }
    if (!this.d.store.getState().storageOk) {
      // A storage error: never sync an in-memory seed over real Homes.
      this.setStatus({ phase: 'error', error: 'Homes could not be read on this computer' }, st)
      return
    }
    this.setStatus({ phase: 'syncing', error: null }, st)
    try {
      if (this.registeredEpoch !== control.epoch) {
        await this.d.transport.register(st.deviceId, this.d.device.name, this.d.device.platform)
        this.registeredEpoch = control.epoch
      }
      st = this.load()
      if (!st.merged) {
        const pull = parsePullAnswer(await this.d.transport.pull(st.deviceId, null))
        this.learn(pull)
        // Sync section: no await from here to the store write.
        const merged = firstSync(this.d.store.getState().homes, pull, this.load(), {
          ...this.d.addresses,
          now: this.d.now(),
          deviceId: st.deviceId,
        })
        st = merged.state
        this.save(st)
        this.applyLocal(st)
      }
      st = this.queueLocal(this.load())
      // Push the outbox, then catch up.
      const sent = Object.values(st.outbox)
      let fullNext = false
      if (sent.length) {
        const ans = (await this.d.transport.push(st.deviceId, sent)) as { rejected?: Array<{ item: unknown }> }
        const acked = ackPush(this.load(), sent, Array.isArray(ans?.rejected) ? ans.rejected : [])
        fullNext = acked.needsFullPull
        this.save(acked.state)
      }
      st = this.load()
      const raw = await this.d.transport.pull(st.deviceId, fullNext ? null : st.cursor)
      const pull: PullAnswer = parsePullAnswer(raw)
      this.learn(pull)
      // Sync section: capture edits made during the awaits, then merge.
      st = this.queueLocal(this.load())
      st = applyPull(st, pull, this.d.now())
      this.save(st)
      this.applyLocal(st)
      // Anything queued while we were away (or by the merge) goes next round.
      if (Object.keys(st.outbox).length) this.again = true
      this.failures = 0
      this.setStatus({ phase: 'synced', error: null }, st)
    } catch (err) {
      const status = statusOf(err)
      if (status === 410) {
        this.markRemoved(this.load())
        return
      }
      if (status === 409) {
        // Not registered (a wiped server DB): register again next round.
        this.registeredEpoch = null
        this.again = this.failures++ < 1
        this.setStatus({ phase: 'error', error: 'This computer is not registered for Homes sync yet' })
        return
      }
      if (status === 403 || status === 404) {
        this.setStatus({ phase: 'unsupported', error: null })
        return
      }
      const delay = BACKOFF_MS[Math.min(this.failures, BACKOFF_MS.length - 1)]
      this.failures++
      this.setStatus({
        phase: status === null ? 'offline' : 'error',
        error: err instanceof Error ? err.message : String(err),
      })
      if (this.running) this.later('retry', () => void this.syncNow(), delay)
    }
  }

  private learn(pull: PullAnswer): void {
    if (pull.serverId) this.d.noteServerId(pull.serverId)
    this.d.noteServerMap?.(pull.servers)
  }

  // ── Settings actions (any window; they write control + HTTP) ─────────

  /** Another window changed the control or the ids map: run a step. */
  poke(): void {
    void this.syncNow()
  }
}

// ── Settings helpers (usable from any window) ────────────────────────

export interface SyncedDevice {
  deviceId: string
  name: string
  platform: string
  lastSyncAt: number | null
  current: boolean
}

export function parseDevices(v: unknown): SyncedDevice[] {
  const list = (v as { devices?: unknown } | null)?.devices
  if (!Array.isArray(list)) throw new Error('homes sync: the device list is not a list')
  return list
    .filter((d): d is Record<string, unknown> => !!d && typeof d === 'object' && typeof (d as { deviceId?: unknown }).deviceId === 'string')
    .map((d) => ({
      deviceId: d.deviceId as string,
      name: typeof d.name === 'string' ? d.name : (d.deviceId as string),
      platform: typeof d.platform === 'string' ? d.platform : '',
      lastSyncAt: typeof d.lastSyncAt === 'number' ? d.lastSyncAt : null,
      current: d.current === true,
    }))
}

/** "Stop syncing": this computer leaves its home server's device list and
 *  keeps its Homes as they are. */
export async function stopSyncing(kv: KeyValue | null, transport: HomesSyncTransport): Promise<void> {
  const c = readControl(kv)
  writeControl(kv, { enabled: false, epoch: c.epoch })
  const st = parseSyncState(kv?.getItem(HOMES_SYNC_STORAGE_KEY) ?? null)
  if (st && transport.supported()) {
    try {
      await transport.remove(st.deviceId)
    } catch (err) {
      // Unknown here already (404) is fine; anything else is reported.
      if (statusOf(err) !== 404) throw err
    }
  }
}

/** "Sync again" / "Start syncing": re-register and re-run the union. */
export function startSyncing(kv: KeyValue | null): void {
  const c = readControl(kv)
  writeControl(kv, { enabled: true, epoch: c.epoch + 1 })
}

// ── Status store (every window) ──────────────────────────────────────

export const useHomesSyncStatus = create<HomesSyncStatus>(() => ({ ...INITIAL_STATUS }))

export function publishStatus(kv: KeyValue | null, s: HomesSyncStatus): void {
  useHomesSyncStatus.setState(s)
  try {
    kv?.setItem(HOMES_SYNC_STATUS_KEY, JSON.stringify(s))
  } catch {
    /* status is best effort */
  }
}

export function readStatus(raw: string | null): HomesSyncStatus | null {
  if (!raw) return null
  try {
    const v = JSON.parse(raw) as Partial<HomesSyncStatus>
    if (!v || typeof v.phase !== 'string') return null
    return { ...INITIAL_STATUS, ...v } as HomesSyncStatus
  } catch {
    return null
  }
}

/** One short line for the badge tooltip and Settings. */
export function statusLine(s: HomesSyncStatus): string {
  const where = s.serverLabel
  switch (s.phase) {
    case 'synced':
      return s.pending > 0 ? `Homes sync with ${where} · ${s.pending} waiting` : `Homes sync with ${where}`
    case 'syncing':
      return `Homes sync with ${where} · syncing…`
    case 'offline':
      return s.pending > 0
        ? `Sync paused · ${s.pending} change${s.pending === 1 ? '' : 's'} waiting for ${where}`
        : `Sync paused · ${where} can’t be reached`
    case 'removed':
      return `This computer was removed from Homes sync on ${where}`
    case 'unsupported':
      return `${where} doesn’t sync Homes (update K2 there)`
    case 'error':
      return `Homes sync with ${where} hit a problem${s.error ? `: ${s.error}` : ''}`
    case 'off':
    default:
      return 'Homes sync is off: Homes stay on this computer'
  }
}
