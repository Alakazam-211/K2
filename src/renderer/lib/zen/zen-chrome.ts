// prd-zen-mode-v1 Z23, Z25 — where the window's own buttons sit while Zen
// owns the whole window (no K2 top bar).
//
// macOS: the system stoplights stay (they can't be hidden, Z24). The Zen
// root publishes the area they take so the page keeps clear of them:
//   --zen-stoplight-safe-left   CSS px from the left where the page may start
//   --zen-stoplight-safe-top    CSS px from the top below the lights
//   --zen-stoplight-rect        "left top width height" (CSS px)
// The left edge reuses the zoom-aware `--k2-stoplight-spacer` the Styles
// traffic-light controller already writes on every zoom change, so the safe
// area follows ⌘= / ⌘- with no Zen code of its own. In S4 the lights still
// sit where the Style put them (`inset`); S5 moves them to the Zen
// `[chrome] stoplight-offset` through one chrome owner.
//
// Linux / Windows: no native chrome; K2 draws its own cluster in Zen
// (`ZenChromeCluster`) and measures it; its rect is published the same way.

import {
  STOPLIGHT_SPACER_VAR,
  TRAFFIC_LIGHT_Y_NUDGE_PX,
  stoplightSpacerPx,
  trafficLightZoom,
} from '@/lib/traffic-lights'
import {
  TOP_BAR_PAD_X_PX,
  TRAFFIC_LIGHT_CLUSTER_GAP_PX,
  TRAFFIC_LIGHT_CLUSTER_RIGHT_PX,
} from '@/lib/desktop-chrome'
import type { ZenRect } from './zen-controls'

/** The macOS title-bar band the lights are centred in (points, unscaled). */
export const MAC_TITLEBAR_BAND_PX = 28

export interface ZenStoplightArea {
  /** CSS custom properties for the Zen root. */
  vars: Record<string, string>
  /** The reserved rect, in CSS px, for the control check. */
  rect: ZenRect
}

/**
 * macOS stoplight area at window inset `inset` (points) and app zoom
 * `zoom`. Native buttons don't scale with CSS zoom; CSS px = points / zoom.
 */
export function macStoplightArea(inset: number, zoom: number | null | undefined): ZenStoplightArea {
  const z = trafficLightZoom(zoom)
  const i = Number.isFinite(inset) && inset > 0 ? inset : 0
  const spacer = stoplightSpacerPx(z, i)
  const lead = i + TOP_BAR_PAD_X_PX
  const safeLeft = lead + spacer + TRAFFIC_LIGHT_CLUSTER_GAP_PX
  const safeTop = (i + TRAFFIC_LIGHT_Y_NUDGE_PX + MAC_TITLEBAR_BAND_PX) / z
  const width = (TRAFFIC_LIGHT_CLUSTER_RIGHT_PX + i) / z
  return {
    vars: {
      // Same sum as the number, but riding the live spacer variable.
      '--zen-stoplight-safe-left': `calc(${lead + TRAFFIC_LIGHT_CLUSTER_GAP_PX}px + var(${STOPLIGHT_SPACER_VAR}))`,
      '--zen-stoplight-safe-top': `${round(safeTop)}px`,
      '--zen-stoplight-rect': `0px 0px ${round(width)}px ${round(safeTop)}px`,
    },
    rect: { left: 0, top: 0, width: round(Math.max(width, safeLeft - TRAFFIC_LIGHT_CLUSTER_GAP_PX)), height: round(safeTop) },
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

/**
 * macOS: the stoplight area right now. S4 reads where the Style put the
 * lights (the window inset) and the app zoom, the same inputs the Styles
 * traffic-light controller reads; S5 swaps the inset for Zen's own
 * `stoplight-offset` through the one chrome owner.
 */
export function currentMacStoplightArea(): ZenStoplightArea {
  let inset = 0
  if (typeof document !== 'undefined') {
    const raw = getComputedStyle(document.documentElement).getPropertyValue('--inset-window').trim()
    inset = Number.parseFloat(raw) || 0
  }
  const zoom = typeof window === 'undefined' ? 1 : (window as { __k2soZoom?: number }).__k2soZoom
  return macStoplightArea(inset, zoom)
}

function round(n: number): number {
  return Math.round(n * 100) / 100
}
