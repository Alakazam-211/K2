// prd-zen-mode-v1 Z2, Q5 (Rosson 2026-10-04 answers 3 and 5) — where Zen
// exists at all.
//
// - Desktop only: the hosted web client and the Companion apps get nothing.
//   `webFeatures.zen` is the flag (`web/features.ts`, like `multiHost`).
// - Windows waits for Wesley's smoke test (G-Win): Zen is built for Windows
//   (Ctrl+Alt+Z, the K2 window controls) but hidden until
//   `ZEN_WINDOWS_ENABLED` flips.
// - Only a main-layout window: a Focus window (`focus-*`) or a ticket window
//   (`window-ticket-*`) never shows Home, so never shows Zen.

import { webFeatures } from '@/web/features'
import { desktopOsFromNavigator, type DesktopOs } from '@/lib/desktop-chrome'
import { getCurrentWindow } from '@tauri-apps/api/window'

/** Flip when the Windows smoke (G-Win) passes. */
export const ZEN_WINDOWS_ENABLED = false

export function currentDesktopOs(): DesktopOs {
  return desktopOsFromNavigator(typeof navigator === 'undefined' ? null : navigator)
}

/** Pure: may Zen exist in a client built `web`, running on `os`? */
export function zenSupportedOn(web: boolean, os: DesktopOs): boolean {
  if (web) return false
  if (os === 'windows') return ZEN_WINDOWS_ENABLED
  return true
}

/** Pure: is this window one that shows Home (not Focus, not a ticket)? */
export function isZenWindow(hash: string, label: string | null): boolean {
  if (/^#(focus|ticket)=/.test(hash)) return false
  if (label && (/^focus-/.test(label) || /^window-ticket-/.test(label))) return false
  return true
}

let windowLabel: string | null | undefined

function readWindowLabel(): string | null {
  if (windowLabel !== undefined) return windowLabel
  try {
    windowLabel = getCurrentWindow().label
  } catch {
    // Not in Tauri (tests, a plain browser): a regular window.
    windowLabel = null
  }
  return windowLabel
}

/** Zen exists in this window of this client. Decided once per page. */
let cached: boolean | null = null
export function zenAvailable(): boolean {
  if (cached !== null) return cached
  const hash = typeof window === 'undefined' ? '' : window.location.hash
  cached = zenSupportedOn(!webFeatures.zen, currentDesktopOs()) && isZenWindow(hash, readWindowLabel())
  return cached
}

/** Tests only: forget the cached decision. */
export function __resetZenAvailableForTests(): void {
  cached = null
  windowLabel = undefined
}
