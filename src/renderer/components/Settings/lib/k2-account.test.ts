import { describe, expect, it } from 'vitest'
import { isApexTunnelLabel } from './k2-account'

describe('isApexTunnelLabel', () => {
  it('accepts single-label apex purchases', () => {
    expect(isApexTunnelLabel('rosson')).toBe(true)
    expect(isApexTunnelLabel('z3thon')).toBe(true)
    expect(isApexTunnelLabel('nsi')).toBe(true)
    expect(isApexTunnelLabel('claimchaser')).toBe(true)
    expect(isApexTunnelLabel('rpmavs')).toBe(true)
  })

  it('rejects nested routing labels (not tunnel roots)', () => {
    expect(isApexTunnelLabel('staging.z3thon')).toBe(false)
    expect(isApexTunnelLabel('api.z3thon')).toBe(false)
    expect(isApexTunnelLabel('rosson.rpmavs')).toBe(false)
    expect(isApexTunnelLabel('rpm.rosson')).toBe(false)
    expect(isApexTunnelLabel('schedule.rpmavs')).toBe(false)
  })

  it('rejects empty or whitespace', () => {
    expect(isApexTunnelLabel('')).toBe(false)
    expect(isApexTunnelLabel('   ')).toBe(false)
    expect(isApexTunnelLabel('ros son')).toBe(false)
  })
})
