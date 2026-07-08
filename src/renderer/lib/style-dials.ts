// Style System P5 — dial resolution (pure).
//
// A dial is a user-adjustable knob a Style's manifest advertises
// (registry: StyleMeta.dials). The user's value for a dial lives in
// localStorage (`k2.dial.<styleId>.<dialId>`, a bare number); this
// module turns that untrusted raw string into the concrete CSS value
// written onto <html>. Pure functions, no DOM/storage access — the
// store (stores/style.ts) and Settings UI own the side effects.

import type { StyleDialMeta } from '@/styles.generated'

/** localStorage key for a dial's user-set value. */
export function dialStorageKey(styleId: string, dialId: string): string {
  return `k2.dial.${styleId}.${dialId}`
}

/** A dial's resting value: its declared default, else its minimum. */
export function dialDefault(dial: StyleDialMeta): number {
  return dial.default ?? dial.min
}

/** Resolve an untrusted stored value (localStorage string) into a
 *  usable number: non-numeric/absent → the dial's default, out-of-range
 *  → clamped to [min, max]. */
export function resolveDialValue(dial: StyleDialMeta, raw: string | null | undefined): number {
  if (raw == null || raw.trim() === '') return dialDefault(dial)
  const n = Number(raw)
  if (!Number.isFinite(n)) return dialDefault(dial)
  return Math.min(dial.max, Math.max(dial.min, n))
}

/** The CSS value a dial writes: the number plus the dial's unit. */
export function formatDialValue(dial: StyleDialMeta, value: number): string {
  return `${value}${dial.unit ?? ''}`
}
