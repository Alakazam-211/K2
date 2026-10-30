// stash-sync-v1 (0.46.7): this device's id for the stash = its Homes
// device id (one device registry per person, `/cli/me/homes/devices*`).
// It lives in the Homes sync state (`k2.homes.sync.v1`); when Homes sync
// hasn't run on this device yet, it is made here exactly as the Homes
// runner would (`emptySyncState`), so both always agree.

import { HOMES_SYNC_STORAGE_KEY } from '@/stores/homes'
import { emptySyncState, parseSyncState } from '@/lib/homes-sync/engine'

export interface DeviceKv {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

function newDeviceId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return `d-${crypto.randomUUID()}`
  return `d-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`
}

function defaultKv(): DeviceKv | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

/** This device's id, or null where storage is unavailable. */
export function stashDeviceId(kv: DeviceKv | null = defaultKv()): string | null {
  if (!kv) return null
  try {
    const st = parseSyncState(kv.getItem(HOMES_SYNC_STORAGE_KEY))
    if (st) return st.deviceId
    const fresh = emptySyncState(newDeviceId())
    kv.setItem(HOMES_SYNC_STORAGE_KEY, JSON.stringify(fresh))
    return fresh.deviceId
  } catch {
    return null
  }
}
