import { describe, expect, it } from 'vitest'

import {
  cellChanged,
  encodeSgrMouse,
  mouseRoute,
  sgrButtonCode,
} from './sgrMouse'

const noMods = { shift: false, alt: false, ctrl: false, meta: false }

describe('mouseRoute', () => {
  it('stays local when the app is not mouse-reporting', () => {
    // claude's normal (inline) mode: both bits false — K2-native
    // selection untouched.
    expect(
      mouseRoute({ mouseReport: false, sgrMouse: false }, noMods),
    ).toBe('local')
    expect(mouseRoute({}, noMods)).toBe('local')
  })

  it('stays local without SGR encoding even when reporting is on', () => {
    // Legacy X10-only apps: high-bit bytes can't ride the JSON text
    // channel — same rule as the wheel branch.
    expect(
      mouseRoute({ mouseReport: true, sgrMouse: false }, noMods),
    ).toBe('local')
  })

  it('forwards a plain gesture when reporting + SGR are on', () => {
    expect(
      mouseRoute({ mouseReport: true, sgrMouse: true }, noMods),
    ).toBe('forward')
  })

  it('cmd bypasses forwarding (link modifier)', () => {
    expect(
      mouseRoute(
        { mouseReport: true, sgrMouse: true },
        { ...noMods, meta: true },
      ),
    ).toBe('local')
  })

  it('shift and option bypass forwarding (local-selection override)', () => {
    expect(
      mouseRoute(
        { mouseReport: true, sgrMouse: true },
        { ...noMods, shift: true },
      ),
    ).toBe('local')
    expect(
      mouseRoute(
        { mouseReport: true, sgrMouse: true },
        { ...noMods, alt: true },
      ),
    ).toBe('local')
  })

  it('ctrl alone still forwards (it is a forwarded modifier, not an override)', () => {
    expect(
      mouseRoute(
        { mouseReport: true, sgrMouse: true },
        { ...noMods, ctrl: true },
      ),
    ).toBe('forward')
  })
})

describe('sgrButtonCode', () => {
  it('maps left/middle/right to 0/1/2', () => {
    expect(sgrButtonCode(0)).toBe(0)
    expect(sgrButtonCode(1)).toBe(1)
    expect(sgrButtonCode(2)).toBe(2)
  })

  it('collapses exotic buttons to left', () => {
    expect(sgrButtonCode(3)).toBe(0)
    expect(sgrButtonCode(4)).toBe(0)
  })
})

describe('encodeSgrMouse', () => {
  it('encodes a left press with M final', () => {
    // The exact byte sequence the study verified moves claude's
    // cursor: \x1b[<0;11;30M.
    expect(encodeSgrMouse(0, 'press', false, 11, 30)).toBe('\x1b[<0;11;30M')
  })

  it('encodes a release with lowercase m and the REAL button code', () => {
    expect(encodeSgrMouse(0, 'release', false, 14, 30)).toBe('\x1b[<0;14;30m')
    expect(encodeSgrMouse(2, 'release', false, 5, 2)).toBe('\x1b[<2;5;2m')
  })

  it('adds +32 for drag motion', () => {
    // Study's verified drag-motion sequence: \x1b[<32;10;30M.
    expect(encodeSgrMouse(0, 'motion', false, 10, 30)).toBe('\x1b[<32;10;30M')
    expect(encodeSgrMouse(1, 'motion', false, 3, 4)).toBe('\x1b[<33;3;4M')
  })

  it('adds +16 for ctrl on press, motion and release', () => {
    expect(encodeSgrMouse(0, 'press', true, 1, 1)).toBe('\x1b[<16;1;1M')
    expect(encodeSgrMouse(0, 'motion', true, 2, 2)).toBe('\x1b[<48;2;2M')
    expect(encodeSgrMouse(0, 'release', true, 3, 3)).toBe('\x1b[<16;3;3m')
  })

  it('encodes middle and right buttons', () => {
    expect(encodeSgrMouse(1, 'press', false, 7, 8)).toBe('\x1b[<1;7;8M')
    expect(encodeSgrMouse(2, 'press', false, 7, 8)).toBe('\x1b[<2;7;8M')
  })
})

describe('cellChanged', () => {
  it('always fires from a null anchor', () => {
    expect(cellChanged(null, { col: 1, row: 1 })).toBe(true)
  })

  it('suppresses same-cell motion', () => {
    expect(cellChanged({ col: 9, row: 30 }, { col: 9, row: 30 })).toBe(false)
  })

  it('fires on a column or row crossing', () => {
    expect(cellChanged({ col: 9, row: 30 }, { col: 10, row: 30 })).toBe(true)
    expect(cellChanged({ col: 9, row: 30 }, { col: 9, row: 29 })).toBe(true)
  })
})
