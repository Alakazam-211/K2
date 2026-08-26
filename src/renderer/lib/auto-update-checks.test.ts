// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest'
import {
  LS_AUTO_UPDATE_CHECKS,
  parseAutoUpdateChecks,
  autoUpdateChecksEnabled,
  setAutoUpdateChecksEnabled,
} from './auto-update-checks'

describe('auto-update-checks localStorage', () => {
  beforeEach(() => {
    localStorage.removeItem(LS_AUTO_UPDATE_CHECKS)
  })

  it('defaults ON when unset', () => {
    expect(parseAutoUpdateChecks(null)).toBe(true)
    expect(autoUpdateChecksEnabled()).toBe(true)
  })

  it('falsy strings disable', () => {
    for (const v of ['0', 'false', 'OFF', 'no']) {
      expect(parseAutoUpdateChecks(v)).toBe(false)
    }
  })

  it('truthy / garbage stay enabled (do not surprise-disable)', () => {
    for (const v of ['1', 'true', 'on', 'yes', 'maybe', '']) {
      expect(parseAutoUpdateChecks(v)).toBe(true)
    }
  })

  it('round-trips off on this install', () => {
    setAutoUpdateChecksEnabled(false)
    expect(localStorage.getItem(LS_AUTO_UPDATE_CHECKS)).toBe('0')
    expect(autoUpdateChecksEnabled()).toBe(false)
    setAutoUpdateChecksEnabled(true)
    expect(localStorage.getItem(LS_AUTO_UPDATE_CHECKS)).toBe('1')
    expect(autoUpdateChecksEnabled()).toBe(true)
  })
})
