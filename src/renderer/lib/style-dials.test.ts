import { describe, expect, it } from 'vitest'
import type { StyleDialMeta } from '@/styles.generated'
import { dialDefault, dialStorageKey, formatDialValue, resolveDialValue } from './style-dials'

const frost: StyleDialMeta = {
  id: 'frost',
  label: 'Frost',
  token: '--material-blur',
  min: 0,
  max: 30,
  step: 1,
  unit: 'px',
  default: 18,
}

/** A community dial that declares no default and no unit. */
const bare: StyleDialMeta = { id: 'bare', label: 'Bare', token: '--x', min: 2, max: 10 }

describe('dialStorageKey', () => {
  it('follows the k2.dial.<styleId>.<dialId> convention', () => {
    expect(dialStorageKey('glass', 'frost')).toBe('k2.dial.glass.frost')
  })
})

describe('dialDefault', () => {
  it('uses the declared default', () => {
    expect(dialDefault(frost)).toBe(18)
  })

  it('falls back to min when no default is declared', () => {
    expect(dialDefault(bare)).toBe(2)
  })
})

describe('resolveDialValue', () => {
  it('parses a stored numeric string', () => {
    expect(resolveDialValue(frost, '7')).toBe(7)
    expect(resolveDialValue(frost, '7.5')).toBe(7.5)
  })

  it('falls back to the default when absent or non-numeric', () => {
    expect(resolveDialValue(frost, null)).toBe(18)
    expect(resolveDialValue(frost, undefined)).toBe(18)
    expect(resolveDialValue(frost, '')).toBe(18)
    expect(resolveDialValue(frost, '  ')).toBe(18)
    expect(resolveDialValue(frost, 'blurry')).toBe(18)
    expect(resolveDialValue(frost, 'NaN')).toBe(18)
    expect(resolveDialValue(frost, 'Infinity')).toBe(18)
  })

  it('clamps out-of-range values to [min, max]', () => {
    expect(resolveDialValue(frost, '-5')).toBe(0)
    expect(resolveDialValue(frost, '999')).toBe(30)
    expect(resolveDialValue(frost, '0')).toBe(0)
    expect(resolveDialValue(frost, '30')).toBe(30)
  })

  it('handles a dial without a declared default (falls back to min)', () => {
    expect(resolveDialValue(bare, null)).toBe(2)
    expect(resolveDialValue(bare, 'junk')).toBe(2)
    expect(resolveDialValue(bare, '11')).toBe(10)
  })
})

describe('formatDialValue', () => {
  it('appends the unit', () => {
    expect(formatDialValue(frost, 18)).toBe('18px')
    expect(formatDialValue(frost, 0)).toBe('0px')
  })

  it('omits the unit when the dial declares none', () => {
    expect(formatDialValue(bare, 4)).toBe('4')
  })
})
