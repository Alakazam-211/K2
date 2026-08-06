/**
 * Hosted web (`VITE_WEB`) feature matrix — desktop-only surfaces amputated.
 *
 * `true` = surface enabled (desktop / Tauri). `false` = hidden or no-op
 * on the hosted SPA. All flags derive from `isWebClient()` so desktop
 * builds stay byte-identical when `VITE_WEB` is unset.
 *
 * Wire high-traffic UI entry points only; do not rewrite the app around
 * this module. See PRD prd-hosted-web-client-and-edge-delivery-v1.md §7 / §9.6.
 */

import { isWebClient } from '@/lib/is-web'

const desktop = !isWebClient()

export const webFeatures = {
  /** Native embedded Browser pane (WKWebView child). */
  browserPane: desktop,
  /** Tauri app auto-updater (check / download / install & relaunch). */
  appUpdater: desktop,
  /**
   * Settings → General → Developer tools (open Chromium DevTools via Tauri).
   * Desktop only — hosted web has no `open_app_devtools` invoke target.
   */
  appDevTools: desktop,
  /** macOS FDA / Accessibility / Mic permissions Settings section. */
  permissions: desktop,
  /** Multi-host ServerSwitcher + Add server (hosted web is single same-origin). */
  multiHost: desktop,
  /**
   * macOS traffic-light (close/min/max) left inset. Desktop reserves ~70px;
   * browser chrome has no window controls — reclaim that space.
   */
  trafficLightSpacer: desktop,
} as const

/** Width (px) reserved for traffic lights; 0 on hosted web. */
export const TRAFFIC_LIGHT_SPACER_PX = webFeatures.trafficLightSpacer ? 70 : 0

export type WebFeatures = typeof webFeatures
