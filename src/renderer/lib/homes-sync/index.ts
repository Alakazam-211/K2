// homes-sync-v1 (0.46.1) — wiring the Homes sync runner into the app.
//
// `startHomesSync()` runs once per window (App mount). Every window
// follows the status (`k2.homes.syncStatus.v1`); only the window holding
// the Web Lock `k2.homes.sync` runs the runner, against THIS computer's
// daemon (the desktop's default home server). Where Web Locks are missing,
// the window runs it itself (a second runner would only repeat idempotent
// pushes and pulls).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import { savedHostForKey } from '@/lib/host-key'
import { HOMES_SYNC_STORAGE_KEY, useHomesStore } from '@/stores/homes'
import { onHomesChanged, onHomesDeviceRemoved } from '@/stores/session-events'
import { isWebClient } from '@/lib/is-web'
import { desktopOsFromNavigator } from '@/lib/desktop-chrome'
import {
  SERVER_IDS_STORAGE_KEY,
  fromWireAddress,
  invalidateServerIds,
  noteServerId,
  noteServerMap,
  onServerIdsChanged,
  toWireAddress,
} from '@/lib/server-ids'
import {
  HOMES_SYNC_CONTROL_KEY,
  HOMES_SYNC_FEATURE,
  HOMES_SYNC_LOCK,
  HOMES_SYNC_STATUS_KEY,
  HomesSyncRunner,
  publishStatus,
  readStatus,
  useHomesSyncStatus,
  type HomesSyncTransport,
  type KeyValue,
} from './runner'
import type { WireItem } from './engine'

function localScope(): ServerScope {
  return scopeForHost('local')
}

/** The desktop's home server: this computer's daemon (owner token). */
export function localTransport(): HomesSyncTransport {
  return {
    supported: () => localScope().serverSupports(HOMES_SYNC_FEATURE),
    pull: (deviceId: string, since: number | null) =>
      daemonCliGet(localScope(), 'me/homes', since === null ? { deviceId } : { deviceId, since }),
    push: (deviceId: string, changes: WireItem[]) => daemonCliPost(localScope(), 'me/homes/push', { deviceId, changes }),
    register: (deviceId: string, name: string, platform: string) =>
      daemonCliPost(localScope(), 'me/homes/devices/register', { deviceId, name, platform }),
    remove: (deviceId: string) => daemonCliPost(localScope(), 'me/homes/devices/remove', { deviceId }),
    listDevices: (deviceId: string) => daemonCliGet(localScope(), 'me/homes/devices', { deviceId }),
  }
}

export function localKv(): KeyValue | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

/** This device as the server lists it: `macos` / `windows` / `linux` /
 *  `web`, and a plain name. */
export function thisDevice(): { name: string; platform: string } {
  if (isWebClient()) return { name: 'K2 in a browser', platform: 'web' }
  const os = desktopOsFromNavigator(typeof navigator === 'undefined' ? null : navigator)
  const platform = os === 'mac' ? 'macos' : os === 'other' ? 'desktop' : os
  const label = os === 'mac' ? 'macOS' : os === 'windows' ? 'Windows' : os === 'linux' ? 'Linux' : 'desktop'
  return { name: `K2 desktop (${label})`, platform }
}

function newDeviceId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return `d-${crypto.randomUUID()}`
  return `d-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`
}

function isSavedHost(hostKey: string): boolean {
  return hostKey === 'local' || savedHostForKey(useConnectHostStore.getState().hosts, hostKey) !== null
}

let started = false
let runner: HomesSyncRunner | null = null

/** The running runner in this window (null when another window runs it). */
export function homesSyncRunner(): HomesSyncRunner | null {
  return runner
}

export function startHomesSync(): () => void {
  if (started || typeof window === 'undefined') return () => {}
  started = true
  const kv = localKv()
  const cleanups: Array<() => void> = []

  // Every window: follow the status the runner publishes.
  const initial = readStatus(kv?.getItem(HOMES_SYNC_STATUS_KEY) ?? null)
  if (initial) useHomesSyncStatus.setState(initial)
  const onStorage = (e: StorageEvent): void => {
    if (e.key === HOMES_SYNC_STATUS_KEY) {
      const s = readStatus(e.newValue)
      if (s) useHomesSyncStatus.setState(s)
    } else if (e.key === SERVER_IDS_STORAGE_KEY) {
      invalidateServerIds()
    } else if (e.key === HOMES_SYNC_CONTROL_KEY) {
      runner?.poke()
    }
  }
  window.addEventListener('storage', onStorage)
  cleanups.push(() => window.removeEventListener('storage', onStorage))

  const run = (): (() => void) => {
    const r = new HomesSyncRunner({
      store: useHomesStore,
      transport: localTransport(),
      kv,
      addresses: {
        toWire: toWireAddress,
        fromWire: (a) => fromWireAddress(a, isSavedHost),
      },
      noteServerId: (id) => noteServerId('local', id),
      noteServerMap,
      now: () => Date.now(),
      newDeviceId,
      device: thisDevice(),
      onStatus: (s) => publishStatus(kv, s),
    })
    runner = r
    const offs: Array<() => void> = []
    offs.push(onHomesChanged(localScope(), (cursor) => r.onRemoteCursor(cursor)))
    offs.push(onHomesDeviceRemoved(localScope(), (id) => r.onDeviceRemoved(id)))
    // A server's id learned (any window): rows move to it, unknown ids resolve.
    offs.push(onServerIdsChanged(() => r.poke()))
    const onFocus = (): void => r.poke()
    window.addEventListener('focus', onFocus)
    window.addEventListener('online', onFocus)
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
      runner = null
    }
  }

  const locks = (navigator as Navigator & { locks?: LockManager }).locks
  if (locks && typeof locks.request === 'function') {
    let release: (() => void) | null = null
    let stopRun: (() => void) | null = null
    void locks
      .request(HOMES_SYNC_LOCK, () => {
        stopRun = run()
        // Hold the lock for this window's life.
        return new Promise<void>((resolve) => {
          release = resolve
        })
      })
      .catch((err: unknown) => console.warn('[homes-sync] lock failed:', err))
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

export { HOMES_SYNC_STORAGE_KEY }
