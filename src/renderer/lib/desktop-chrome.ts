/**
 * Desktop window chrome flags — single owner for traffic-light spacer,
 * app Menu button, and custom min/max/close controls.
 *
 * Hosted web: no chrome. macOS desktop: system traffic lights + menu bar.
 * Windows/Linux desktop: frameless chrome with Menu + window controls.
 */

import { isWebClient } from '@/lib/is-web'

export type DesktopChrome = {
  trafficLightSpacer: boolean
  appMenuButton: boolean
  windowControls: boolean
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

/** Align with stores/style.ts macOS detection. */
export function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false
  const platform = navigator.platform?.toLowerCase() ?? ''
  if (platform.includes('mac')) return true
  const ua = navigator.userAgent?.toLowerCase() ?? ''
  return ua.includes('mac os') || ua.includes('macintosh')
}

export function getDesktopChrome(): DesktopChrome {
  if (isWebClient()) {
    return {
      trafficLightSpacer: false,
      appMenuButton: false,
      windowControls: false,
    }
  }
  if (isMacPlatform()) {
    return {
      trafficLightSpacer: true,
      appMenuButton: false,
      windowControls: false,
    }
  }
  return {
    trafficLightSpacer: false,
    appMenuButton: true,
    windowControls: true,
  }
}

/** Effective traffic-light inset (0 on web / Win / Linux). */
export const TRAFFIC_LIGHT_SPACER_PX = getDesktopChrome().trafficLightSpacer
  ? TRAFFIC_LIGHT_SPACER_BASE_PX
  : 0
