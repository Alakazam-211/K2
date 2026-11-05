import { describe, expect, it } from 'vitest'
import { isApexTunnelLabel } from './k2-account'

describe('isApexTunnelLabel', () => {
  it('accepts single-label apex purchases', () => {
    expect(isApexTunnelLabel('rosson')).toBe(true)
    expect(isApexTunnelLabel('z3thon')).toBe(true)
    expect(isApexTunnelLabel('acme')).toBe(true)
    expect(isApexTunnelLabel('cyberdyne')).toBe(true)
    expect(isApexTunnelLabel('rekall')).toBe(true)
  })

  it('rejects nested routing labels (not tunnel roots)', () => {
    expect(isApexTunnelLabel('staging.z3thon')).toBe(false)
    expect(isApexTunnelLabel('api.z3thon')).toBe(false)
    expect(isApexTunnelLabel('rosson.rekall')).toBe(false)
    expect(isApexTunnelLabel('rpm.rosson')).toBe(false)
    expect(isApexTunnelLabel('schedule.rekall')).toBe(false)
  })

  it('rejects empty or whitespace', () => {
    expect(isApexTunnelLabel('')).toBe(false)
    expect(isApexTunnelLabel('   ')).toBe(false)
    expect(isApexTunnelLabel('ros son')).toBe(false)
  })
})
