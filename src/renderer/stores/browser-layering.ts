// How Browser tab pages sit with K2's own windows and menus (0.46.17).
//
// - `underlay` (macOS, the default there): the page is a native view BELOW
//   K2's UI. Menus, popovers and toasts over a page leave it live, and
//   clicks, scroll and hover over the page reach it
//   (src-tauri commands/browser_page_layers.rs, wry's `underlay`).
// - `legacy`: today's path. The page floats over K2, and anything K2 draws
//   over it freezes it (a picture shows, the native view hides).
//
// Windows and Linux always use `legacy` (they keep cut-outs).
//
// A client setting, not a server one: this install's localStorage
// (`k2.browserLayering`), like `k2.chatLinkCopyButton`. If the underlay
// fails to set up at run time, this window drops to `legacy` for the rest
// of the session and says why in the console; the setting is left alone.
//
// `k2.browserHoverPrivateWebkitFallback` ('1' = on, off by default): the
// named private WebKit hover route (`hover_private_webkit_fallback`), for a
// macOS where the public route fails.

import { create } from 'zustand'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { webFeatures } from '@/web/features'

export const LS_BROWSER_LAYERING = 'k2.browserLayering'
export const LS_BROWSER_HOVER_PRIVATE_FALLBACK = 'k2.browserHoverPrivateWebkitFallback'

export type BrowserLayering = 'underlay' | 'legacy'

/** The default for this system: underlay on macOS, legacy elsewhere. */
export function defaultBrowserLayering(mac: boolean): BrowserLayering {
  return mac ? 'underlay' : 'legacy'
}

/** The stored choice; unset or garbage reads as this system's default. */
export function readBrowserLayering(mac: boolean = isMacPlatform()): BrowserLayering {
  try {
    if (typeof localStorage === 'undefined') return defaultBrowserLayering(mac)
    const v = localStorage.getItem(LS_BROWSER_LAYERING)
    return v === 'underlay' || v === 'legacy' ? v : defaultBrowserLayering(mac)
  } catch {
    return defaultBrowserLayering(mac)
  }
}

export function writeBrowserLayering(mode: BrowserLayering): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.setItem(LS_BROWSER_LAYERING, mode)
  } catch {
    // Blocked storage: the in-memory value holds for this session.
  }
}

export function readHoverPrivateFallback(): boolean {
  try {
    if (typeof localStorage === 'undefined') return false
    return localStorage.getItem(LS_BROWSER_HOVER_PRIVATE_FALLBACK) === '1'
  } catch {
    return false
  }
}

interface BrowserLayeringState {
  /** The person's choice (or this system's default). */
  mode: BrowserLayering
  /** Why the underlay was dropped this session, or null. */
  failed: string | null
  hoverPrivateFallback: boolean
  setMode: (mode: BrowserLayering) => void
  /** The underlay failed at run time: legacy until K2 restarts. */
  markFailed: (reason: string) => void
}

export const useBrowserLayeringStore = create<BrowserLayeringState>((set, get) => ({
  mode: readBrowserLayering(),
  failed: null,
  hoverPrivateFallback: readHoverPrivateFallback(),
  setMode: (mode) => {
    writeBrowserLayering(mode)
    set({ mode })
  },
  markFailed: (reason) => {
    if (get().failed) return
    console.warn(`[browser-underlay] falling back to the legacy browser path: ${reason}`)
    set({ failed: reason })
  },
}))

/** Is the underlay in use: macOS, desktop app, chosen, and not failed. */
export function underlayActive(
  s: Pick<BrowserLayeringState, 'mode' | 'failed'>,
  mac: boolean = isMacPlatform(),
  desktop: boolean = webFeatures.browserPane,
): boolean {
  return mac && desktop && s.mode === 'underlay' && s.failed === null
}

/** React: is the underlay in use in this window right now. */
export function useBrowserUnderlay(): boolean {
  const mode = useBrowserLayeringStore((s) => s.mode)
  const failed = useBrowserLayeringStore((s) => s.failed)
  return underlayActive({ mode, failed })
}
