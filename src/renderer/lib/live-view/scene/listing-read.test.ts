// Reading K2 Render's scene listing and still meta (S7 calls these; S4's
// overlay and details read their shapes): an object's bounds with `approx`
// a bool and the why in `approx_reason`, `hidden` and `hidden_by`; the
// still's camera and a listing camera's display values, whose keys are the
// fixture's until K2 Cores' camera read lands, so they're read in this one
// place; and the response meta's counts.

import { describe, expect, it } from 'vitest'
import { listingCameraFrom, objectListingFrom, stillCameraFrom, stillMetaFrom } from './listing-read'

const I16 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, -5, 1]

describe('an object in the listing', () => {
  it('reads bounds, approx as a bool with its reason, and hidden with what hides it', () => {
    expect(objectListingFrom({ bounds: { min: [0, 0, 0], max: [1, 2, 3], approx: true, approx_reason: 'modifiers' }, hidden: true, hidden_by: 'collection' })).toEqual({ min: [0, 0, 0], max: [1, 2, 3], approx: true, approx_reason: 'modifiers', hidden: true, hidden_by: 'collection' })
    expect(objectListingFrom({ bounds: { min: [0, 0, 0], max: [1, 1, 1], approx: false } })).toEqual({ min: [0, 0, 0], max: [1, 1, 1], approx: false, approx_reason: null, hidden: false, hidden_by: null })
  })

  it('takes only a bool: the old string form is not approximate', () => {
    expect(objectListingFrom({ bounds: { min: [0, 0, 0], max: [1, 1, 1], approx: 'modifiers' } })!.approx).toBe(false)
  })

  it('has no box without whole bounds', () => {
    expect(objectListingFrom({ hidden: true })).toBeNull()
    expect(objectListingFrom({ bounds: { min: [0, 0], max: [1, 1, 1] } })).toBeNull()
    expect(objectListingFrom(null)).toBeNull()
  })
})

describe('cameras (the fixture’s keys, read only here)', () => {
  it('a still’s camera: view and proj of sixteen numbers, and its display values', () => {
    expect(stillCameraFrom({ view: I16, proj: I16, fx: 1200, fy: 1200, near: 0.05, shift_x: 0.1, shift_y: 0 })).toEqual({ view: I16, proj: I16, fx: 1200, fy: 1200, near: 0.05, shift_x: 0.1, shift_y: 0 })
    expect(stillCameraFrom({ view: I16.slice(1), proj: I16 })).toBeNull()
    expect(stillCameraFrom({ view: I16, proj: [...I16.slice(1), Number.NaN] })).toBeNull()
    expect(stillCameraFrom(undefined)).toBeNull()
  })

  it('a camera in the listing: active, lens and sensor', () => {
    expect(listingCameraFrom({ active: true, lens_mm: 35, sensor_mm: [36, 24], sensor_fit: 'auto' })).toEqual({ active: true, lensMm: 35, sensorMm: [36, 24], sensorFit: 'auto' })
    // the wire's sensor is [width, height] (extras.rs camera_display; k2-mpc-blend WIRE.md): a bare number isn't it
    expect(listingCameraFrom({ sensor_mm: 36 }).sensorMm).toBeNull()
    expect(listingCameraFrom({})).toEqual({ active: false, lensMm: null, sensorMm: null, sensorFit: null })
  })
})

describe('the still’s response meta', () => {
  it('reads the counts and flags, missing ones as nothing', () => {
    expect(stillMetaFrom({ base_values: true, hidden_left_out: true, objects_hidden: 3, objects_not_evaluated: 1 })).toEqual({ base_values: true, hidden_left_out: true, objects_hidden: 3, objects_not_evaluated: 1 })
    expect(stillMetaFrom({})).toEqual({ base_values: false, hidden_left_out: false, objects_hidden: 0, objects_not_evaluated: 0 })
    expect(stillMetaFrom({ objects_hidden: -2, objects_not_evaluated: 'x' })).toEqual({ base_values: false, hidden_left_out: false, objects_hidden: 0, objects_not_evaluated: 0 })
  })
})
