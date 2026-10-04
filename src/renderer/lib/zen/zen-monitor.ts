// prd-zen-mode-v1 Z28 — when K2 checks the required controls: after first
// paint, again 1.5 s later, after every `zen_changed` re-render (a new page
// version), after a window resize, and every 10 s while the window is
// focused. Two failed checks in a row are a failure (`createControlStreak`).
//
// Each mounted Zen page registers its check here; `runZenControlChecksNow`
// runs every registered check at once (the schedule's tick, and tests).

import type { ZenRect } from './zen-controls'
import { domZenGeometry, type ZenGeometry } from './zen-controls'

export const ZEN_CHECK_SECOND_MS = 1500
export const ZEN_CHECK_INTERVAL_MS = 10_000

let geometry: ZenGeometry = domZenGeometry

/** The geometry the checks read (layout; tests swap it). */
export function zenGeometry(): ZenGeometry {
  return geometry
}

/** Tests only: read layout from `next` (jsdom has none). */
export function __setZenGeometryForTests(next: ZenGeometry | null): void {
  geometry = next ?? domZenGeometry
}

const checks = new Set<() => void>()

/** Register one page's check. Returns the unregister. */
export function registerZenControlCheck(check: () => void): () => void {
  checks.add(check)
  return () => void checks.delete(check)
}

/** Run every mounted page's check now. */
export function runZenControlChecksNow(): void {
  for (const c of [...checks]) c()
}

/** Reserved rects provider (stoplights / K2's cluster), set by the root. */
let reserved: () => readonly ZenRect[] = () => []

export function setZenReservedRects(fn: () => readonly ZenRect[]): () => void {
  reserved = fn
  return () => {
    if (reserved === fn) reserved = () => []
  }
}

export function zenReservedRects(): readonly ZenRect[] {
  return reserved()
}
