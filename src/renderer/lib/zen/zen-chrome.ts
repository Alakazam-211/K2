// prd-zen-mode-v1 Z23, Z24, Z25, Z49–Z51 — the window's own chrome while
// Zen owns the whole window (no K2 top bar).
//
// macOS: the system stoplights stay (they can't be hidden, Z24/Z50). Zen's
// `[chrome]` table picks their shape (`round` / `square`), nudges them
// (`stoplight-offset = [x, y]`, 0–24 px) and picks the window corners
// (`system` / `square`, Z51). Those values reach AppKit through the same
// native hooks the Styles use, via the one chrome owner in `stores/style.ts`
// (`setChromeSource`): held while Zen is shown, the Style's restored on
// leaving. The Zen root publishes the area the lights take so the page keeps
// clear of them:
//   --zen-stoplight-safe-left   CSS px from the left where the page may start
//   --zen-stoplight-safe-top    CSS px from the top below the lights
//   --zen-stoplight-rect        "left top width height" (CSS px)
// computed with the same math as `lib/traffic-lights.ts` (native buttons
// don't scale with the app zoom; CSS px = points / zoom). The Styles'
// zoom-aware `--k2-stoplight-spacer` keeps describing the Style (Z49).
//
// Linux / Windows: no native chrome; K2 draws its own cluster in Zen
// (`ZenChromeCluster`) and measures it; its rect is published the same way.
// The `[chrome]` table changes nothing there.

import { TRAFFIC_LIGHT_Y_NUDGE_PX, ZEN_STOPLIGHT_INSET_PX, trafficLightZoom } from '@/lib/traffic-lights'
import { TRAFFIC_LIGHT_CLUSTER_GAP_PX, TRAFFIC_LIGHT_CLUSTER_RIGHT_PX } from '@/lib/desktop-chrome'
import type { ZenChromeSource } from '@/stores/style'
import type { ZenRect } from './zen-controls'

/** The macOS title-bar band the lights are centred in (points, unscaled). */
export const MAC_TITLEBAR_BAND_PX = 28

/** `[chrome] stoplight-offset` range, px (daemon `STOPLIGHT_OFFSET_MAX`). */
export const ZEN_STOPLIGHT_OFFSET_MAX = 24

/** K2's default Zen chrome (the daemon's built-in `themes/default.toml` `[chrome]`). */
export const ZEN_DEFAULT_CHROME: ZenChromeSource = Object.freeze({
  corners: 'system',
  stoplights: 'round',
  offset: Object.freeze([0, 0]) as unknown as [number, number],
}) as ZenChromeSource

/**
 * The `chrome` block of `/cli/zen/get` → the chrome owner's input. Only the
 * known keys and values are read; anything else (an unknown corner style,
 * `hidden` stoplights, an offset out of range) keeps the last good value for
 * that key, else K2's default. The daemon reports the error.
 */
export function parseZenChrome(raw: unknown, lastGood: ZenChromeSource = ZEN_DEFAULT_CHROME): ZenChromeSource {
  const o = raw !== null && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  const corners = o.corners === 'system' || o.corners === 'square' ? o.corners : lastGood.corners
  const stoplights = o.stoplights === 'round' || o.stoplights === 'square' ? o.stoplights : lastGood.stoplights
  const off = o['stoplight-offset']
  const okNum = (n: unknown): n is number =>
    typeof n === 'number' && Number.isFinite(n) && n >= 0 && n <= ZEN_STOPLIGHT_OFFSET_MAX
  const offset: [number, number] =
    Array.isArray(off) && off.length === 2 && okNum(off[0]) && okNum(off[1])
      ? [off[0], off[1]]
      : [lastGood.offset[0], lastGood.offset[1]]
  return { corners, stoplights, offset }
}

export interface ZenStoplightArea {
  /** CSS custom properties for the Zen root. */
  vars: Record<string, string>
  /** The reserved rect, in CSS px, for the control check. */
  rect: ZenRect
}

/**
 * macOS stoplight area with Zen's `stoplight-offset` (points) at app zoom
 * `zoom`. Native buttons don't scale with CSS zoom; CSS px = points / zoom.
 * Zen moves the lights `ZEN_STOPLIGHT_INSET_PX` down and right first
 * (Rosson 2026-10-04), so at offset [0, 0] and 100% they end at 77 px, the
 * page may start at 91 px, and the band below them starts at 39 px.
 */
export function macStoplightArea(
  offset: readonly [number, number],
  zoom: number | null | undefined,
): ZenStoplightArea {
  const z = trafficLightZoom(zoom)
  const x = (Number.isFinite(offset[0]) && offset[0] > 0 ? offset[0] : 0) + ZEN_STOPLIGHT_INSET_PX
  const y = (Number.isFinite(offset[1]) && offset[1] > 0 ? offset[1] : 0) + ZEN_STOPLIGHT_INSET_PX
  const width = (TRAFFIC_LIGHT_CLUSTER_RIGHT_PX + x) / z
  const safeLeft = width + TRAFFIC_LIGHT_CLUSTER_GAP_PX
  const safeTop = (y + TRAFFIC_LIGHT_Y_NUDGE_PX + MAC_TITLEBAR_BAND_PX) / z
  return {
    vars: {
      '--zen-stoplight-safe-left': `${round(safeLeft)}px`,
      '--zen-stoplight-safe-top': `${round(safeTop)}px`,
      '--zen-stoplight-rect': `0px 0px ${round(width)}px ${round(safeTop)}px`,
    },
    rect: { left: 0, top: 0, width: round(width), height: round(safeTop) },
  }
}

/** Linux / Windows: the area K2's own cluster takes (measured). */
export function clusterArea(rect: ZenRect | null, side: 'left' | 'right', viewportWidth: number): ZenStoplightArea {
  const r = rect ?? { left: 0, top: 0, width: 0, height: 0 }
  const safeLeft = side === 'left' ? r.left + r.width : 0
  return {
    vars: {
      '--zen-stoplight-safe-left': `${round(safeLeft)}px`,
      '--zen-stoplight-safe-top': `${round(r.top + r.height)}px`,
      '--zen-stoplight-safe-right': side === 'right' ? `${round(Math.max(0, viewportWidth - r.left))}px` : '0px',
      '--zen-stoplight-rect': `${round(r.left)}px ${round(r.top)}px ${round(r.width)}px ${round(r.height)}px`,
    },
    rect: r,
  }
}

/** macOS: the stoplight area right now for Zen chrome `chrome`. */
export function currentMacStoplightArea(chrome: ZenChromeSource): ZenStoplightArea {
  const zoom = typeof window === 'undefined' ? 1 : (window as { __k2soZoom?: number }).__k2soZoom
  return macStoplightArea(chrome.offset, zoom)
}

function round(n: number): number {
  return Math.round(n * 100) / 100
}
