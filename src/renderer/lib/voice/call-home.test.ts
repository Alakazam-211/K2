// @vitest-environment jsdom
//
// One call, one home window: the owner token and its election.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  CALL_FOCUS_KEY,
  CALL_HOME_KEY,
  CALL_HOME_LEASE_MS,
  clearCallHome,
  decideHome,
  forgetWindow,
  noteWindowFocus,
  readCallHome,
  readWindowFocus,
  writeCallHome,
} from './call-home'

const CALL = 'sewlab.k2.dev|dm:42'

beforeEach(() => localStorage.clear())
afterEach(() => localStorage.clear())

describe('the owner token', () => {
  it('is kept per client, and junk reads as none', () => {
    expect(readCallHome()).toBeNull()
    writeCallHome({ call: CALL, owner: 'main', until: 9_000 })
    expect(readCallHome()).toEqual({ call: CALL, owner: 'main', until: 9_000 })
    localStorage.setItem(CALL_HOME_KEY, '{"call":1}')
    expect(readCallHome()).toBeNull()
  })

  it("is cleared only for its own call", () => {
    writeCallHome({ call: CALL, owner: 'main', until: 9_000 })
    clearCallHome('sewlab.k2.dev|room-1')
    expect(readCallHome()?.owner).toBe('main')
    clearCallHome(CALL)
    expect(readCallHome()).toBeNull()
  })

  it('remembers when each window last had focus, and forgets a window that goes', () => {
    noteWindowFocus('main', 100)
    noteWindowFocus('window-2', 200)
    expect(readWindowFocus()).toEqual({ main: 100, 'window-2': 200 })
    forgetWindow('window-2')
    expect(readWindowFocus()).toEqual({ main: 100 })
    localStorage.setItem(CALL_FOCUS_KEY, 'junk')
    expect(readWindowFocus()).toEqual({})
  })
})

describe('the election', () => {
  const base = { call: CALL, now: 10_000, prefer: null, never: ['window-call'] }

  it('with no owner, the window focused last takes it (where the call was started or answered)', () => {
    expect(decideHome({ ...base, home: null, live: ['main', 'window-2'], focus: { main: 5, 'window-2': 9 } })).toBe('window-2')
    // Never focused here yet: the first live window by name, so every window agrees.
    expect(decideHome({ ...base, home: null, live: ['window-2', 'main'], focus: {} })).toBe('main')
  })

  it('an owner that is still open keeps it, even past its lease (a background window renews late)', () => {
    const home = { call: CALL, owner: 'main', until: 1_000 }
    expect(decideHome({ ...base, home, live: ['main', 'window-2'], focus: { 'window-2': 9_999 } })).toBe('main')
  })

  it('an owner that closed keeps it until its lease runs out, then the window focused last takes over', () => {
    const home = { call: CALL, owner: 'window-2', until: 10_000 + CALL_HOME_LEASE_MS }
    const live = ['main', 'focus-a']
    const focus = { main: 3, 'focus-a': 7, 'window-2': 9 }
    expect(decideHome({ ...base, home, live, focus })).toBe('window-2')
    expect(decideHome({ ...base, now: 10_001 + CALL_HOME_LEASE_MS, home, live, focus })).toBe('focus-a')
  })

  it("another call's token counts for nothing", () => {
    const home = { call: 'other|room', owner: 'window-2', until: 99_999 }
    expect(decideHome({ ...base, home, live: ['main', 'window-2'], focus: { main: 5 } })).toBe('main')
  })

  it("the call's own window is never elected; a preferred window (the one it came from) is, while open", () => {
    expect(decideHome({ ...base, home: null, live: ['main', 'window-call'], focus: { 'window-call': 99 } })).toBe('main')
    expect(decideHome({ ...base, home: null, prefer: 'window-2', live: ['main', 'window-2'], focus: { main: 99 } })).toBe('window-2')
    expect(decideHome({ ...base, home: null, prefer: 'window-9', live: ['main', 'window-2'], focus: { 'window-2': 1, main: 0 } })).toBe('window-2')
  })

  it("the call's own window, closed by the OS, gives it up at once: the window it came from takes it back", () => {
    const home = { call: CALL, owner: 'window-call', until: 10_000 + CALL_HOME_LEASE_MS }
    const input = { ...base, home, prefer: 'window-2', live: ['main', 'window-2'], focus: { main: 9 } }
    expect(decideHome(input)).toBe('window-call')
    expect(decideHome({ ...input, closed: ['window-call'] })).toBe('window-2')
  })

  it('no window left: nobody', () => {
    const home = { call: CALL, owner: 'main', until: 0 }
    expect(decideHome({ ...base, home, live: [], focus: { main: 5 } })).toBeNull()
  })

  it("without the app's window list (one window), the lease and the focus list decide", () => {
    const home = { call: CALL, owner: 'main', until: 20_000 }
    expect(decideHome({ ...base, home, live: null, focus: {} })).toBe('main')
    expect(decideHome({ ...base, home: { ...home, until: 1 }, live: null, focus: { 'window-2': 3 } })).toBe('window-2')
  })
})
