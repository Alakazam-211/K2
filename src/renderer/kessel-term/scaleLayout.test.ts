// S7b — pinned scale-to-fit decision table (extracted pure function).
//
// Pins the contract the pane's scaleLayout useMemo rides on:
//  - identity guards (no snapshot / unmeasured cells / zero box)
//  - active viewer, unpinned: centered 1:1 (clips), hold-and-scale
//  - passive viewer, unpinned: letterbox at max(fit, 0.4)
//  - PINNED: letterbox REGARDLESS of isActiveViewer, floor 0.25,
//    centered 1:1 when the pinned grid fits, wins over pendingResize.

import { describe, it, expect } from 'vitest'
import {
  computeScaleLayout,
  PASSIVE_SCALE_FLOOR,
  PINNED_SCALE_FLOOR,
  type ScaleLayoutInput,
} from './scaleLayout'

/** 10×20px cells over a 100×30 grid = 1000×600 grid px. */
function input(overrides: Partial<ScaleLayoutInput> = {}): ScaleLayoutInput {
  return {
    snapCols: 100,
    snapRows: 30,
    cellWidth: 10,
    cellHeight: 20,
    // avail = container - 4 ⇒ 1004/604 gives avail exactly 1000×600.
    containerWidth: 1004,
    containerHeight: 604,
    isActiveViewer: true,
    pinned: false,
    pendingResize: null,
    ...overrides,
  }
}

describe('computeScaleLayout — guards', () => {
  it('returns identity when the snapshot or cell metrics are missing', () => {
    const identity = { scale: 1, offsetX: 0, offsetY: 0, passive: false }
    expect(computeScaleLayout(input({ snapCols: 0 }))).toEqual(identity)
    expect(computeScaleLayout(input({ snapRows: 0 }))).toEqual(identity)
    expect(computeScaleLayout(input({ cellWidth: 0 }))).toEqual(identity)
    expect(computeScaleLayout(input({ cellHeight: 0 }))).toEqual(identity)
  })

  it('returns identity for a zero-sized box', () => {
    expect(computeScaleLayout(input({ containerWidth: 4 }))).toEqual({
      scale: 1,
      offsetX: 0,
      offsetY: 0,
      passive: false,
    })
  })
})

describe('computeScaleLayout — unpinned (existing behavior)', () => {
  it('active viewer with an exactly-fitting grid renders centered 1:1', () => {
    const r = computeScaleLayout(input())
    expect(r).toEqual({ scale: 1, offsetX: 0, offsetY: 0, passive: false })
  })

  it('active viewer with an OVERSIZE grid still renders 1:1 (clips) — its resizes drive the PTY', () => {
    const r = computeScaleLayout(
      input({ containerWidth: 504, containerHeight: 304 }),
    )
    expect(r.scale).toBe(1)
    expect(r.passive).toBe(false)
  })

  it('passive viewer letterboxes an oversize grid at the fit scale', () => {
    // avail 500×300 vs grid 1000×600 ⇒ fit 0.5.
    const r = computeScaleLayout(
      input({
        isActiveViewer: false,
        containerWidth: 504,
        containerHeight: 304,
      }),
    )
    expect(r.scale).toBe(0.5)
    expect(r.passive).toBe(true)
    // Perfectly proportional fit ⇒ no leftover to center.
    expect(r.offsetX).toBe(0)
    expect(r.offsetY).toBe(0)
  })

  it('passive floor stays 0.4: a tiny box clips instead of shrinking below it', () => {
    // avail 100×60 vs grid 1000×600 ⇒ fit 0.1 → floored to 0.4.
    const r = computeScaleLayout(
      input({
        isActiveViewer: false,
        containerWidth: 104,
        containerHeight: 64,
      }),
    )
    expect(r.scale).toBe(PASSIVE_SCALE_FLOOR)
    expect(r.passive).toBe(true)
    // Floored above fit ⇒ grid overflows the box; offsets clamp at 0.
    expect(r.offsetX).toBe(0)
    expect(r.offsetY).toBe(0)
  })

  it('active viewer with a pending resize letterboxes the OLD grid (hold-and-scale)', () => {
    const r = computeScaleLayout(
      input({
        containerWidth: 504,
        containerHeight: 304,
        pendingResize: { cols: 50, rows: 15 },
      }),
    )
    expect(r.scale).toBe(0.5)
    expect(r.passive).toBe(false)
  })
})

describe('computeScaleLayout — pinned (S7b)', () => {
  it('the ACTIVE viewer letterboxes when the pinned grid overflows its box (no centered-1:1 clip)', () => {
    // avail 500×300 vs pinned grid 1000×600 ⇒ fit 0.5.
    const r = computeScaleLayout(
      input({
        pinned: true,
        isActiveViewer: true,
        containerWidth: 504,
        containerHeight: 304,
      }),
    )
    expect(r.scale).toBe(0.5)
    // Pinned never reports `passive` — the pin badge is the affordance.
    expect(r.passive).toBe(false)
  })

  it('a pinned grid that FITS renders centered 1:1 with symmetric whole-px gutters', () => {
    // avail 1200×800 vs grid 1000×600 ⇒ slack 200×200 → 100/100.
    const r = computeScaleLayout(
      input({
        pinned: true,
        containerWidth: 1204,
        containerHeight: 804,
      }),
    )
    expect(r).toEqual({ scale: 1, offsetX: 100, offsetY: 100, passive: false })
  })

  it('pinned floor is 0.25: fits between 0.25 and 0.4 are honored, not clipped at 0.4', () => {
    // avail 300×180 vs grid 1000×600 ⇒ fit 0.3 — below the passive
    // floor, above the pinned one.
    const r = computeScaleLayout(
      input({
        pinned: true,
        containerWidth: 304,
        containerHeight: 184,
      }),
    )
    expect(r.scale).toBeCloseTo(0.3, 10)
  })

  it('pinned floor clamps a sub-0.25 fit to 0.25', () => {
    // avail 100×60 ⇒ fit 0.1 → floored to 0.25.
    const r = computeScaleLayout(
      input({
        pinned: true,
        containerWidth: 104,
        containerHeight: 64,
      }),
    )
    expect(r.scale).toBe(PINNED_SCALE_FLOOR)
  })

  it('pinned applies to the passive viewer too (same math, lower floor)', () => {
    const r = computeScaleLayout(
      input({
        pinned: true,
        isActiveViewer: false,
        containerWidth: 104,
        containerHeight: 64,
      }),
    )
    expect(r.scale).toBe(PINNED_SCALE_FLOOR)
    expect(r.passive).toBe(false)
  })

  it('pinned wins over a stale pendingResize hold', () => {
    // While pinned no resize is emitted, so any leftover hold must
    // not stretch the clamped grid past the pinned letterbox.
    const r = computeScaleLayout(
      input({
        pinned: true,
        containerWidth: 504,
        containerHeight: 304,
        pendingResize: { cols: 50, rows: 15 },
      }),
    )
    expect(r.scale).toBe(0.5)
    expect(r.passive).toBe(false)
  })

  it('letterbox offsets center the scaled pinned grid on the slack axis', () => {
    // avail 1200×300 vs grid 1000×600 ⇒ fit min(1.2, 0.5) = 0.5.
    // Scaled grid 500×300 ⇒ offsetX (1200-500)/2 = 350, offsetY 0
    // (the constraining axis is exact by construction).
    const r = computeScaleLayout(
      input({
        pinned: true,
        containerWidth: 1204,
        containerHeight: 304,
      }),
    )
    expect(r.scale).toBeCloseTo(0.5, 10)
    expect(r.offsetX).toBeCloseTo(350, 10)
    expect(r.offsetY).toBeCloseTo(0, 10)
  })
})
