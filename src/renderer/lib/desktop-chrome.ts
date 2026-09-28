/**
 * Desktop window chrome flags — single owner for the traffic-light
 * spacer, the Linux square stoplights, and Windows min/max/close.
 *
 * Hosted web: no chrome. macOS: system lights + menu bar (logo opens
 * the dashboard). Windows: window controls, no Menu button (the logo
 * opens the menu). Linux: in-flow square stoplights, no spacer and no
 * window controls (a spacer here is an empty gap). The logo opens the
 * menu on Windows and Linux.
 */

import { isWebClient } from '@/lib/is-web'

export type DesktopChrome = {
  trafficLightSpacer: boolean
  /** Separate "Menu" button. Always false — the logo owns that menu. */
  appMenuButton: boolean
  /** Windows frameless min/max/close. False on Linux (squares instead). */
  windowControls: boolean
  /** In-flow close / minimize / maximize squares. Linux only. */
  linuxStoplights: boolean
}

export type DesktopOs = 'mac' | 'windows' | 'linux' | 'other'

const NO_CHROME: DesktopChrome = {
  trafficLightSpacer: false,
  appMenuButton: false,
  windowControls: false,
  linuxStoplights: false,
}

/**
 * macOS 27 system lights at inset 0, measured in-process: each button is
 * 14×14, origins at 9 / 32 / 55, so the zoom frame ends at 69. The top bar's
 * px-3 (12) sits outside this spacer, so the spacer is 69 − 12. The flex gap
 * after the zoom light, and between the logo, server switcher, and page tabs,
 * is 14 (the 9px light gap plus the 5px Rosson asked to add).
 */
export const TRAFFIC_LIGHT_CLUSTER_RIGHT_PX = 69
export const TRAFFIC_LIGHT_CLUSTER_GAP_PX = 14
const TOP_BAR_PAD_X_PX = 12

/** Reserved width for macOS traffic lights when the spacer is active. */
export const TRAFFIC_LIGHT_SPACER_BASE_PX =
  TRAFFIC_LIGHT_CLUSTER_RIGHT_PX - TOP_BAR_PAD_X_PX

/** Min width for the App "Menu" button cluster. */
export const APP_MENU_BUTTON_MIN_WIDTH_PX = 52

/** Approximate width of min · max · close (3 × 24px — Rosson preferred density). */
export const WINDOW_CONTROLS_WIDTH_PX = 72

type NavLike = { platform?: string; userAgent?: string }

/** mac check matches the previous isMacPlatform (platform or UA). */
export function desktopOsFromNavigator(nav: NavLike | null | undefined): DesktopOs {
  const platform = nav?.platform?.toLowerCase() ?? ''
  const ua = nav?.userAgent?.toLowerCase() ?? ''
  if (platform.includes('mac') || ua.includes('mac os') || ua.includes('macintosh')) return 'mac'
  if (platform.includes('win') || ua.includes('windows')) return 'windows'
  if (platform.includes('linux') || ua.includes('linux')) return 'linux'
  return 'other'
}

/** Align with stores/style.ts macOS detection, plus the UA fallback. */
export function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false
  return desktopOsFromNavigator(navigator) === 'mac'
}

export function desktopChromeFor(web: boolean, os: DesktopOs): DesktopChrome {
  if (web) return { ...NO_CHROME }
  if (os === 'mac') return { ...NO_CHROME, trafficLightSpacer: true }
  if (os === 'linux') return { ...NO_CHROME, linuxStoplights: true }
  return { ...NO_CHROME, windowControls: true }
}

/** Win/Linux (and any other non-mac desktop). Not hosted web, not macOS. */
export function logoOpensAppMenu(web: boolean, os: DesktopOs): boolean {
  return !web && os !== 'mac'
}

/** Same min-width the Agents bar uses: spacer or the retired Menu button. */
export function topBarLeftClusterMinWidth(
  chrome: DesktopChrome = getDesktopChrome(),
): number | undefined {
  if (chrome.trafficLightSpacer) return TRAFFIC_LIGHT_SPACER_BASE_PX + 60
  if (chrome.appMenuButton) return APP_MENU_BUTTON_MIN_WIDTH_PX + 60
  return undefined
}

export function getDesktopChrome(): DesktopChrome {
  const nav = typeof navigator === 'undefined' ? null : navigator
  return desktopChromeFor(isWebClient(), desktopOsFromNavigator(nav))
}

/** Effective traffic-light inset (0 on web / Win / Linux). */
export const TRAFFIC_LIGHT_SPACER_PX = getDesktopChrome().trafficLightSpacer
  ? TRAFFIC_LIGHT_SPACER_BASE_PX
  : 0
