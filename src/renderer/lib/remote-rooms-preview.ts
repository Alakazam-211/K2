// Home — "Open agents from other servers here" (prd-home-multi-server-client
// MS55; prd-home-seamless-0432 item 1: Z1, Z2, Z4; Rosson 2026-10-03 Q1, Q2).
//
// On: a Home row on another server opens that server's room in Home's main
// area without switching (M4/M5). Off: the row switches the window's server
// (H15 / P1.1). On by default from 0.43.2 on macOS and Linux, and on every
// OS (Windows included) from 0.45.5 (Rosson 2026-10-11). The setting stays
// in Settings → General → Experimental as an opt-out.
//
// Per-computer view preference, never a daemon setting: localStorage
// `k2.homeRemoteRooms.v1`, like `k2.workspaceSwitchFocus`. Every window on
// this computer shares it, so a change in one window reaches the others
// through the `storage` event.
//
// What the key means (Z2, Q1 — an explicit choice is kept):
//
//   | stored value          | reads as                                       |
//   |-----------------------|------------------------------------------------|
//   | `'1'`                 | on (this person turned it on), on every OS     |
//   | `'0'`                 | off (this person turned it off), on every OS   |
//   | missing               | the default: on, on every OS (0.45.5; before   |
//   |                       | that Windows and an unknown OS read off)       |
//   | blocked storage       | the platform default; a toggle holds for this  |
//   |                       | window's session only                          |
//   | the hosted web client | always off (`webFeatures.multiHost`, MS56)     |
//
// Only the toggle writes the key, and only `'1'` or `'0'` (that was true
// before 0.43.2 too: `'0'` has always meant someone turned it off). The
// default is never written, so people who never touched the switch follow
// the default, including a later flip of the Windows default. That is how
// 0.45.5 turns it on for Windows without a migration: a stored `'0'` is
// always someone's explicit off (kept), a missing key is "never touched"
// (now reads on).
//
// The migration (`migrateRemoteRoomsPreview`, once per window at load):
// `'1'` and `'0'` are kept as they are. Any other stored value (blank,
// `'true'`, junk) was never written by the toggle, read as off before
// 0.43.2, and is not anyone's choice, so it is removed: that computer then
// reads the platform default, like one that never touched the switch.

import { create } from 'zustand'
import { webFeatures } from '@/web/features'
import { desktopOsFromNavigator, type DesktopOs } from '@/lib/desktop-chrome'

export const LS_REMOTE_ROOMS_PREVIEW = 'k2.homeRemoteRooms.v1'

/**
 * The default for people who never touched the switch, per OS. Q2 (Rosson
 * 2026-10-03) turned it on for macOS and Linux in 0.43.2; Rosson 2026-10-11
 * turned it on everywhere in 0.45.5 so people start to see these menus.
 * An explicit `'0'` still wins on every OS.
 */
export const REMOTE_ROOMS_DEFAULT_ON: Readonly<Record<DesktopOs, boolean>> = {
  mac: true,
  linux: true,
  windows: true,
  other: true,
}

/** The minimal storage surface (localStorage shape) this module uses. */
export interface PreviewStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

function defaultStorage(): PreviewStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    // Storage blocked (private mode): the platform default applies.
    return null
  }
}

/** This computer's OS, from the webview's navigator. */
export function currentDesktopOs(): DesktopOs {
  return typeof navigator === 'undefined' ? 'other' : desktopOsFromNavigator(navigator)
}

/** `'1'` on, `'0'` off; anything else (missing included) is the default
 *  for `os`. */
export function parseRemoteRoomsPreview(raw: string | null, os: DesktopOs = currentDesktopOs()): boolean {
  if (raw === '1') return true
  if (raw === '0') return false
  return REMOTE_ROOMS_DEFAULT_ON[os]
}

/** Is the switch on? Always false on the hosted web client. */
export function readRemoteRoomsPreview(
  storage: PreviewStorage | null = defaultStorage(),
  os: DesktopOs = currentDesktopOs(),
): boolean {
  if (!webFeatures.multiHost) return false
  if (!storage) return REMOTE_ROOMS_DEFAULT_ON[os]
  try {
    return parseRemoteRoomsPreview(storage.getItem(LS_REMOTE_ROOMS_PREVIEW), os)
  } catch (err) {
    console.warn('[remote-rooms-preview] read failed:', err)
    return REMOTE_ROOMS_DEFAULT_ON[os]
  }
}

export type RemoteRoomsMigration = 'kept' | 'untouched' | 'cleared'

/** 0.43.2 migration (see the header): keep `'1'` / `'0'`, leave a missing
 *  key missing, remove anything else. Never writes the default. */
export function migrateRemoteRoomsPreview(storage: PreviewStorage | null = defaultStorage()): RemoteRoomsMigration {
  if (!storage) return 'untouched'
  try {
    const raw = storage.getItem(LS_REMOTE_ROOMS_PREVIEW)
    if (raw === null) return 'untouched'
    if (raw === '1' || raw === '0') return 'kept'
    storage.removeItem(LS_REMOTE_ROOMS_PREVIEW)
    return 'cleared'
  } catch (err) {
    console.warn('[remote-rooms-preview] migration failed:', err)
    return 'untouched'
  }
}

interface RemoteRoomsPreviewState {
  enabled: boolean
  setEnabled(next: boolean): void
}

function initialEnabled(): boolean {
  migrateRemoteRoomsPreview()
  return readRemoteRoomsPreview()
}

export const useRemoteRoomsPreviewStore = create<RemoteRoomsPreviewState>((set) => ({
  enabled: initialEnabled(),
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

/** React hook: are agents from other servers opened here? Re-renders on
 *  change. */
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
