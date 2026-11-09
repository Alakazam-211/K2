// Glass's Frost fallback (prd-themes-terminal-glass-v1 §3.2 G5/G6): reduced
// transparency, or no working backdrop-filter, turns Frost into a solid tile.
// Decided by what this webview reports, never by which OS it is. The result is
// stamped as <html data-frost="tile">, and overrides.css does the rest.

import type { DesktopOs } from './desktop-chrome'
import { desktopOsFromNavigator } from './desktop-chrome'

/** The one switch for G6: add a platform here to ship Glass tile-only on it
 *  when its webview can't draw Frost properly. Empty until the real-webview
 *  gate names one. */
export const TILE_ONLY_PLATFORMS: readonly DesktopOs[] = []

const REDUCED_TRANSPARENCY = '(prefers-reduced-transparency: reduce)'

export function backdropFilterWorks(): boolean {
  if (typeof CSS === 'undefined' || typeof CSS.supports !== 'function') return false
  return CSS.supports('backdrop-filter', 'blur(1px)') || CSS.supports('-webkit-backdrop-filter', 'blur(1px)')
}

export function prefersReducedTransparency(): boolean {
  try {
    return typeof window.matchMedia === 'function' && window.matchMedia(REDUCED_TRANSPARENCY).matches
  } catch {
    return false
  }
}

export function frostIsTileOnly(forced: readonly DesktopOs[] = TILE_ONLY_PLATFORMS): boolean {
  if (typeof navigator !== 'undefined' && forced.includes(desktopOsFromNavigator(navigator))) return true
  return !backdropFilterWorks() || prefersReducedTransparency()
}

/** Stamp the current answer on <html>. Safe to call repeatedly. */
export function stampFrostSupport(): void {
  if (typeof document === 'undefined') return
  const html = document.documentElement
  if (frostIsTileOnly()) html.setAttribute('data-frost', 'tile')
  else html.removeAttribute('data-frost')
}

/** Stamp now, and again whenever the OS setting changes. Once per window. */
export function installFrostSupport(): void {
  stampFrostSupport()
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
  const mq = window.matchMedia(REDUCED_TRANSPARENCY)
  if (typeof mq.addEventListener === 'function') mq.addEventListener('change', stampFrostSupport)
}
