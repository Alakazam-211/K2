// Home M4 — the "Remote rooms (preview)" switch (prd-home-multi-server-client
// MS55, answer Q3: Settings → General → Experimental, off by default).
//
// Off (default): a Home row on another server switches the window's server,
// as today (H15 / P1.1). On: the row opens that server's room in Home's main
// area without switching (M4). Turning it on by default needs M5's smoke
// test, Rosson's go, and the Windows check (Q8).
//
// Per-computer view preference, never a daemon setting: localStorage
// `k2.homeRemoteRooms.v1`, like `k2.workspaceSwitchFocus`. Every window on
// this computer shares it, so a change in one window reaches the others
// through the `storage` event. The hosted web client is single-server
// (`webFeatures.multiHost` false, MS56): there it always reads off.

import { create } from 'zustand'
import { webFeatures } from '@/web/features'

export const LS_REMOTE_ROOMS_PREVIEW = 'k2.homeRemoteRooms.v1'

/** The minimal storage surface (localStorage shape) this module uses. */
export interface PreviewStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

function defaultStorage(): PreviewStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    // Storage blocked (private mode): the switch reads off.
    return null
  }
}

/** Only `'1'` is on. Missing, blank or anything else is off (the default). */
export function parseRemoteRoomsPreview(raw: string | null): boolean {
  return raw === '1'
}

/** Is the switch on? Always false on the hosted web client. */
export function readRemoteRoomsPreview(storage: PreviewStorage | null = defaultStorage()): boolean {
  if (!webFeatures.multiHost) return false
  if (!storage) return false
  try {
    return parseRemoteRoomsPreview(storage.getItem(LS_REMOTE_ROOMS_PREVIEW))
  } catch (err) {
    console.warn('[remote-rooms-preview] read failed:', err)
    return false
  }
}

interface RemoteRoomsPreviewState {
  enabled: boolean
  setEnabled(next: boolean): void
}

export const useRemoteRoomsPreviewStore = create<RemoteRoomsPreviewState>((set) => ({
  enabled: readRemoteRoomsPreview(),
  setEnabled(next) {
    if (!webFeatures.multiHost) {
      set({ enabled: false })
      return
    }
    const storage = defaultStorage()
    if (storage) {
      try {
        storage.setItem(LS_REMOTE_ROOMS_PREVIEW, next ? '1' : '0')
      } catch (err) {
        // Blocked storage: the switch still holds for this window's session.
        console.warn('[remote-rooms-preview] write failed:', err)
      }
    }
    set({ enabled: next })
  },
}))

/** React hook: is "Remote rooms (preview)" on? Re-renders on change. */
export function useRemoteRoomsPreview(): boolean {
  return useRemoteRoomsPreviewStore((s) => s.enabled)
}

/** Non-reactive read for click handlers (a Home row deciding between
 *  "switch the window" and "open a room"). */
export function remoteRoomsPreviewEnabled(): boolean {
  return useRemoteRoomsPreviewStore.getState().enabled
}

/** Another window changed the switch: pick it up. Returns the uninstall. */
export function installRemoteRoomsPreviewSync(
  target: Pick<Window, 'addEventListener' | 'removeEventListener'>,
): () => void {
  const onStorage = (e: StorageEvent): void => {
    // `key === null` is a storage.clear() in another window.
    if (e.key !== null && e.key !== LS_REMOTE_ROOMS_PREVIEW) return
    const next = readRemoteRoomsPreview()
    if (useRemoteRoomsPreviewStore.getState().enabled !== next) {
      useRemoteRoomsPreviewStore.setState({ enabled: next })
    }
  }
  target.addEventListener('storage', onStorage)
  return () => target.removeEventListener('storage', onStorage)
}

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  installRemoteRoomsPreviewSync(window)
}
