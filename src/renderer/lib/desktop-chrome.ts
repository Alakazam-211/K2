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

/** Reserved width for macOS traffic lights when spacer is active. */
export const TRAFFIC_LIGHT_SPACER_BASE_PX = 70

/** Min width for the App "Menu" button cluster. */
export const APP_MENU_BUTTON_MIN_WIDTH_PX = 52

/** Approximate width of min · max · close controls (icons nudge left of edge). */
export const WINDOW_CONTROLS_WIDTH_PX = 138

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
