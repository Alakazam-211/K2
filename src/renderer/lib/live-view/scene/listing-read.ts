// Reading K2 Render's scene listing and a still's response meta into the
// tab's shapes (K2 Render's scene listing corrections, 2026-11-16):
// - an object: `bounds {min, max, approx}` with `approx` a BOOL and the why
//   in `approx_reason` ("modifiers" | "driven"), and `hidden: true` with
//   `hidden_by`;
// - the still's camera (`camera {view[16], proj[16], fx, fy, near, shift_x,
//   shift_y}`) and a listing camera's display values (`active, lens_mm,
//   sensor_mm, sensor_fit`). These keys are the fixture's guesses until
//   K2 Cores' camera read lands: this file is the one place that reads
//   them, so the swap is here;
// - the response meta: `base_values`, `hidden_left_out`, `objects_hidden`,
//   `objects_not_evaluated`.
// Defensive: anything malformed reads as nothing, never as a wrong box.

import type { ObjectBounds, StillMeta } from './picture-source'
import type { StillCamera } from './project'

type Raw = Record<string, unknown>
const obj = (v: unknown): Raw | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Raw) : null)
const nums = (v: unknown, n: number): number[] | null => (Array.isArray(v) && v.length === n && v.every((x) => typeof x === 'number' && Number.isFinite(x)) ? (v as number[]) : null)
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const count = (v: unknown): number => (typeof v === 'number' && Number.isInteger(v) && v > 0 ? v : 0)

/** One object of the listing: its local bounds (none: no box), whether they're approximate and why, and whether it's hidden. */
export function objectListingFrom(raw: unknown): ObjectBounds | null {
  const o = obj(raw)
  const b = obj(o?.bounds)
  const min = nums(b?.min, 3)
  const max = nums(b?.max, 3)
  if (!o || !b || !min || !max) return null
  const reason = b.approx_reason ?? o.approx_reason
  return {
    min,
    max,
    approx: b.approx === true,
    approx_reason: reason === 'modifiers' || reason === 'driven' ? reason : null,
    hidden: o.hidden === true,
    hidden_by: typeof o.hidden_by === 'string' ? o.hidden_by : null,
  }
}

/** The camera that drew a still (the fixture's keys). */
export function stillCameraFrom(raw: unknown): StillCamera | null {
  const c = obj(raw)
  const view = nums(c?.view, 16)
  const proj = nums(c?.proj, 16)
  if (!c || !view || !proj) return null
  const out: StillCamera = { view, proj }
  for (const k of ['fx', 'fy', 'near', 'shift_x', 'shift_y'] as const) {
    const v = num(c[k])
    if (v !== null) out[k] = v
  }
  return out
}

export interface ListingCamera {
  active: boolean
  lensMm: number | null
  /** The sensor's size, [width, height] in mm, as the file stores it. */
  sensorMm: number[] | null
  sensorFit: string | null
}

/** A camera's display values in the listing (the fixture's keys). */
export function listingCameraFrom(raw: unknown): ListingCamera {
  const c = obj(raw) ?? {}
  return { active: c.active === true, lensMm: num(c.lens_mm), sensorMm: nums(c.sensor_mm, 2), sensorFit: typeof c.sensor_fit === 'string' ? c.sensor_fit : null }
}

/** A still's response meta: what was left out of it. */
export function stillMetaFrom(raw: unknown): StillMeta {
  const m = obj(raw) ?? {}
  return { base_values: m.base_values === true, hidden_left_out: m.hidden_left_out === true, objects_hidden: count(m.objects_hidden), objects_not_evaluated: count(m.objects_not_evaluated) }
}
