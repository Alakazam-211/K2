// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest'
import {
  LS_AUTO_UPDATE_CHECKS,
  LS_LAST_UPDATE_CHECK,
  msUntilUpdateCheckDue,
  parseLastUpdateCheck,
  readLastUpdateCheck,
  rememberUpdateCheckResult,
  rememberUpdateCheckStarted,
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

describe('last update check', () => {
  const HOUR = 60 * 60 * 1000
  const NOW = 10 * HOUR
  const OFFER = { version: '9.9.9', notes: 'shiny', downloadUrl: null, canInstall: false }

  beforeEach(() => {
    localStorage.removeItem(LS_LAST_UPDATE_CHECK)
  })

  it('no record: due now', () => {
    expect(readLastUpdateCheck(NOW)).toBeNull()
    expect(msUntilUpdateCheckDue(3 * HOUR, NOW)).toBe(0)
  })

  it('counts down from the recorded time, and is due at exactly one interval', () => {
    rememberUpdateCheckStarted(NOW)
    expect(msUntilUpdateCheckDue(3 * HOUR, NOW)).toBe(3 * HOUR)
    expect(msUntilUpdateCheckDue(3 * HOUR, NOW + HOUR)).toBe(2 * HOUR)
    expect(msUntilUpdateCheckDue(3 * HOUR, NOW + 3 * HOUR)).toBe(0)
    expect(msUntilUpdateCheckDue(3 * HOUR, NOW + 4 * HOUR)).toBe(0)
  })

  it('a start keeps the offer found earlier; a result keeps the start time', () => {
    rememberUpdateCheckStarted(NOW)
    rememberUpdateCheckResult(OFFER, NOW + 5)
    expect(readLastUpdateCheck(NOW + 5)).toEqual({ at: NOW, offer: OFFER })
    rememberUpdateCheckStarted(NOW + HOUR)
    expect(readLastUpdateCheck(NOW + HOUR)).toEqual({ at: NOW + HOUR, offer: OFFER })
    rememberUpdateCheckResult(null, NOW + HOUR + 5)
    expect(readLastUpdateCheck(NOW + HOUR + 5)).toEqual({ at: NOW + HOUR, offer: null })
  })

  it('an offer that would not read back is dropped and the time stays, so no refetch on every load', () => {
    rememberUpdateCheckStarted(NOW)
    rememberUpdateCheckResult({ ...OFFER, version: 'latest' }, NOW + 5)
    expect(readLastUpdateCheck(NOW + 5)).toEqual({ at: NOW, offer: null })
  })

  it('a future time or garbage reads as nothing', () => {
    for (const raw of [
      'yesterday',
      'null',
      '5',
      '{}',
      '{"at":null,"offer":null}',
      '{"at":-1,"offer":null}',
      `{"at":${NOW},"offer":"9.9.9"}`,
      `{"at":${NOW + 1},"offer":null}`,
      `{"at":${NOW},"offer":{"version":"9.9.9; rm","notes":null,"downloadUrl":null,"canInstall":false}}`,
      `{"at":${NOW},"offer":{"version":"9.9.9","notes":null,"downloadUrl":"https://github.com/other/K2/releases/x","canInstall":true}}`,
    ]) {
      expect(parseLastUpdateCheck(raw, NOW), raw).toBeNull()
    }
  })

  it('reads a good record, Linux package offer included', () => {
    const offer = {
      version: '9.9.9',
      notes: null,
      downloadUrl: 'https://github.com/Alakazam-211/K2/releases/download/v9.9.9/k2-9.9.9-x86_64.pkg.tar.zst',
      canInstall: true,
    }
    expect(parseLastUpdateCheck(JSON.stringify({ at: NOW, offer }), NOW)).toEqual({ at: NOW, offer })
  })
})
