// stash-sync-v1 (0.46.7) — wiring the prompt stash sync runner into the app.
//
// `startStashSync()` runs once per window (App mount). Only the window
// holding the Web Lock `k2.stash.sync` runs the runner, against THIS
// computer's daemon (the desktop's home server, as for Homes); the others
// mark their changes dirty in localStorage, which the runner hears through
// the `storage` event. Where Web Locks are missing, the window runs it
// itself (a second runner would only repeat idempotent pushes and pulls).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { HOMES_SYNC_CONTROL_KEY, parseDevices } from '@/lib/homes-sync/runner'
import { thisDevice } from '@/lib/homes-sync'
import { onHomesDeviceRemoved, onStashChanged } from '@/stores/session-events'
import { applyStashSync, readStashFile, setStashLocalChangeListener } from '@/stores/prompt-stash'
import { readAllStashRecords } from './idb'
import { stashDeviceId } from './device'
import type { RestoreDeps } from './restore'
import { STASH_SYNC_FEATURE, STASH_SYNC_STATE_KEY } from './sync-engine'
import { StashSyncRunner, useStashSyncStatus, type StashSyncTransport } from './sync-runner'

export const STASH_SYNC_LOCK = 'k2.stash.sync'

function localScope(): ServerScope {
  return scopeForHost('local')
}

function kv(): Storage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

export function stashTransport(): StashSyncTransport {
  return {
    supported: () => localScope().serverSupports(STASH_SYNC_FEATURE),
    pull: (deviceId, since) => daemonCliGet(localScope(), 'me/stash', since === null ? { deviceId } : { deviceId, since }),
    push: (deviceId, changes) => daemonCliPost(localScope(), 'me/stash/push', { deviceId, changes }),
    register: (deviceId, name, platform) => daemonCliPost(localScope(), 'me/homes/devices/register', { deviceId, name, platform }),
  }
}

function statusOf(err: unknown): number | null {
  const s = (err as { status?: unknown } | null)?.status
  return typeof s === 'number' ? s : null
}

/** The restore helpers for a composer on this computer: device names from
 *  the Homes registry, paths checked on this computer's daemon, bytes from
 *  this device's IndexedDB. */
export function stashRestoreDeps(): RestoreDeps {
  const deviceId = stashDeviceId()
  return {
    deviceId,
    deviceNames: async () => {
      const raw = await daemonCliGet(localScope(), 'me/homes/devices', deviceId ? { deviceId } : {})
      return new Map(parseDevices(raw).map((d) => [d.deviceId, d.name]))
    },
    pathExists: async (path) => {
      try {
        await daemonCliGet(localScope(), 'fs/stat', { path })
        return true
      } catch (err) {
        // The daemon answered (not found, refused): it isn't there for us.
        // No answer (offline): we can't tell.
        return statusOf(err) === null ? null : false
      }
    },
    readBytes: readStashFile,
  }
}

let started = false

export function startStashSync(): () => void {
  if (started || typeof window === 'undefined') return () => {}
  started = true
  const cleanups: Array<() => void> = []

  const run = (): (() => void) => {
    const r = new StashSyncRunner({
      transport: stashTransport(),
      local: { apply: applyStashSync, readAll: readAllStashRecords },
      kv: kv(),
      deviceId: () => stashDeviceId(),
      device: thisDevice(),
      onStatus: (s) => useStashSyncStatus.setState(s),
    })
    const offs: Array<() => void> = []
    setStashLocalChangeListener(() => r.localChanged())
    offs.push(() => setStashLocalChangeListener(null))
    offs.push(onStashChanged(localScope(), (cursor) => r.onRemoteCursor(cursor)))
    offs.push(onHomesDeviceRemoved(localScope(), (id) => r.onDeviceRemoved(id)))
    const onStorage = (e: StorageEvent): void => {
      // Another window saved or deleted; Homes "Sync again" / "Stop syncing".
      if (e.key === STASH_SYNC_STATE_KEY) r.localChanged()
      else if (e.key === HOMES_SYNC_CONTROL_KEY) r.poke()
    }
    const onFocus = (): void => r.poke()
    window.addEventListener('storage', onStorage)
    window.addEventListener('focus', onFocus)
    window.addEventListener('online', onFocus)
    offs.push(() => window.removeEventListener('storage', onStorage))
    offs.push(() => window.removeEventListener('focus', onFocus))
    offs.push(() => window.removeEventListener('online', onFocus))
    const tick = setInterval(() => {
      if (typeof document === 'undefined' || document.visibilityState === 'visible') r.poke()
    }, 60_000)
    offs.push(() => clearInterval(tick))
    r.start()
    return () => {
      for (const off of offs) off()
      r.stop()
    }
  }

  const locks = (navigator as Navigator & { locks?: LockManager }).locks
  if (locks && typeof locks.request === 'function') {
    let release: (() => void) | null = null
    let stopRun: (() => void) | null = null
    void locks
      .request(STASH_SYNC_LOCK, () => {
        stopRun = run()
        return new Promise<void>((resolve) => {
          release = resolve
        })
      })
      .catch((err: unknown) => console.warn('[stash-sync] lock failed:', err))
    cleanups.push(() => {
      stopRun?.()
      release?.()
    })
  } else {
    cleanups.push(run())
  }

  return () => {
    for (const c of cleanups) c()
    started = false
  }
}
