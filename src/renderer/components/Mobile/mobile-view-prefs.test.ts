// @vitest-environment jsdom
// The Mobile tab's view prefs, fail loud: live touch is on by default
// (0.46.7) and an explicit off stays off; the latency readout is off by
// default.

import { beforeEach, describe, expect, it } from 'vitest'
import { readMobileLatencyHud, readMobileLiveTouch, writeMobileLatencyHud, writeMobileLiveTouch } from './mobile-view-prefs'

const KEY = 'k2.mobile.liveTouch'

beforeEach(() => localStorage.clear())

describe('live touch pref', () => {
  it('is on by default', () => {
    expect(localStorage.getItem(KEY)).toBeNull()
    expect(readMobileLiveTouch()).toBe(true)
  })

  it('an explicit off is stored as "0" and stays off', () => {
    writeMobileLiveTouch(false)
    expect(localStorage.getItem(KEY)).toBe('0')
    expect(readMobileLiveTouch()).toBe(false)
    writeMobileLiveTouch(true)
    expect(localStorage.getItem(KEY)).toBe('1')
    expect(readMobileLiveTouch()).toBe(true)
  })

  it('the pre-0.46.7 opt-in ("1") stays on', () => {
    localStorage.setItem(KEY, '1')
    expect(readMobileLiveTouch()).toBe(true)
  })
})

describe('latency readout pref', () => {
  it('is off by default and remembers on', () => {
    expect(readMobileLatencyHud()).toBe(false)
    writeMobileLatencyHud(true)
    expect(readMobileLatencyHud()).toBe(true)
    writeMobileLatencyHud(false)
    expect(localStorage.getItem('k2.mobile.latencyHud')).toBeNull()
  })
})
