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
  /** macOS FDA / Accessibility / Mic permissions Settings section. */
  permissions: desktop,
  /** Multi-host ServerSwitcher + Add server (hosted web is single same-origin). */
  multiHost: desktop,
} as const

export type WebFeatures = typeof webFeatures
