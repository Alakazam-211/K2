// Per-window drawer / navbar-rail chrome. Thin-client view state — not
// daemon SSOT. No `storage` event peer-sync (that would re-mirror windows).

import { getCurrentWindow } from '@tauri-apps/api/window'
import { isWebClient } from '@/lib/is-web'

export const WINDOW_CHROME_KEY_PREFIX = 'k2.windowChrome.'

export type WindowChrome = {
  leftOpen: boolean
  rightOpen: boolean
  sidebarCollapsed: boolean
}

export const DEFAULT_WINDOW_CHROME: WindowChrome = {
  leftOpen: true,
  rightOpen: true,
  sidebarCollapsed: false,
}

export type DaemonChromeSeed = {
  leftPanelOpen?: boolean
  rightPanelOpen?: boolean
  sidebarCollapsed?: boolean
}

export function windowChromeKey(label: string): string {
  return `${WINDOW_CHROME_KEY_PREFIX}${label}`
}

export function isFocusWindowLabel(label: string): boolean {
  return label.startsWith('focus-')
}

export function getWindowLabel(): string {
  try {
    const label = getCurrentWindow().label
    return typeof label === 'string' && label.length > 0 ? label : 'main'
  } catch {
    return 'main'
  }
}

function chromeStorage(): Storage | null {
  try {
    if (isWebClient()) {
      return typeof sessionStorage === 'undefined' ? null : sessionStorage
    }
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function asBool(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback
}

function parseChrome(raw: string): WindowChrome | null {
  try {
    const parsed = JSON.parse(raw) as unknown
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return null
    }
    const rec = parsed as Record<string, unknown>
    return {
      leftOpen: asBool(rec.leftOpen, DEFAULT_WINDOW_CHROME.leftOpen),
      rightOpen: asBool(rec.rightOpen, DEFAULT_WINDOW_CHROME.rightOpen),
      sidebarCollapsed: asBool(
        rec.sidebarCollapsed,
        DEFAULT_WINDOW_CHROME.sidebarCollapsed,
      ),
    }
  } catch {
    return null
  }
}

function persist(chrome: WindowChrome, label: string): void {
  const storage = chromeStorage()
  if (!storage) return
  try {
    storage.setItem(
      windowChromeKey(label),
      JSON.stringify({
        leftOpen: chrome.leftOpen,
        rightOpen: chrome.rightOpen,
        sidebarCollapsed: chrome.sidebarCollapsed,
      }),
    )
  } catch {
    // Private mode / quota — memory still updates at the caller.
  }
}

export function hasWindowChromeKey(label = getWindowLabel()): boolean {
  const storage = chromeStorage()
  if (!storage) return false
  return storage.getItem(windowChromeKey(label)) != null
}

export function readWindowChrome(label = getWindowLabel()): WindowChrome | null {
  const storage = chromeStorage()
  if (!storage) return null
  const raw = storage.getItem(windowChromeKey(label))
  if (raw == null) return null
  return parseChrome(raw)
}

export function writeWindowChrome(
  patch: Partial<WindowChrome>,
  label = getWindowLabel(),
): WindowChrome {
  const current = readWindowChrome(label) ?? { ...DEFAULT_WINDOW_CHROME }
  const next: WindowChrome = {
    leftOpen: asBool(patch.leftOpen, current.leftOpen),
    rightOpen: asBool(patch.rightOpen, current.rightOpen),
    sidebarCollapsed:
      isFocusWindowLabel(label) || typeof patch.sidebarCollapsed !== 'boolean'
        ? current.sidebarCollapsed
        : patch.sidebarCollapsed,
  }
  persist(next, label)
  return next
}

/** Seed from daemon only when the per-label key is missing. An early
 *  toggle that created the key wins. Focus labels never take
 *  `sidebarCollapsed` from the daemon. */
export function seedWindowChromeIfMissing(
  daemon: DaemonChromeSeed,
  label = getWindowLabel(),
): WindowChrome {
  if (hasWindowChromeKey(label)) {
    return readWindowChrome(label) ?? { ...DEFAULT_WINDOW_CHROME }
  }
  const next: WindowChrome = {
    leftOpen: asBool(daemon.leftPanelOpen, DEFAULT_WINDOW_CHROME.leftOpen),
    rightOpen: asBool(daemon.rightPanelOpen, DEFAULT_WINDOW_CHROME.rightOpen),
    sidebarCollapsed: isFocusWindowLabel(label)
      ? DEFAULT_WINDOW_CHROME.sidebarCollapsed
      : asBool(daemon.sidebarCollapsed, DEFAULT_WINDOW_CHROME.sidebarCollapsed),
  }
  persist(next, label)
  return next
}
