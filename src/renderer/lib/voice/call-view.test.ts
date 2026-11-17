// @vitest-environment jsdom
//
// The call's view as this client keeps it: the full-screen chord, the call's
// window's frame and clock.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { HOTKEYS, getDefaultKeybindings } from '@shared/hotkeys'
import {
  CALL_CLOCK_KEY,
  CALL_EXPAND_HOTKEY,
  CALL_WINDOW_FRAME_KEY,
  callStartedAt,
  isCallExpandChord,
  isCallWindow,
  noteCallConnected,
  readWindowFrame,
  writeWindowFrame,
} from './call-view'

beforeEach(() => localStorage.clear())
afterEach(() => localStorage.clear())


describe('the full-screen chord', () => {
  const key = (k: Partial<KeyboardEvent> & { getModifierState?: (m: string) => boolean }) => ({
    key: 'v',
    code: 'KeyV',
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...k,
  })

  it('is in the shortcut registry, rebindable, and clashes with no other default', () => {
    const def = HOTKEYS.find((h) => h.id === CALL_EXPAND_HOTKEY)
    expect(def?.defaultKey).toBe('Ctrl+Meta+V')
    expect(def?.defaultKeyOther).toBe('Ctrl+Alt+V')
    for (const mac of [true, false]) {
      const all = Object.values(getDefaultKeybindings(mac))
      expect(all.filter((c) => c === getDefaultKeybindings(mac)[CALL_EXPAND_HOTKEY])).toHaveLength(1)
    }
  })

  it('⌃⌘V on macOS, Ctrl+Alt+V elsewhere; plain paste never counts', () => {
    expect(isCallExpandChord(key({ ctrlKey: true, metaKey: true }), {}, true)).toBe(true)
    expect(isCallExpandChord(key({ metaKey: true }), {}, true)).toBe(false)
    expect(isCallExpandChord(key({ ctrlKey: true, altKey: true }), {}, false)).toBe(true)
    expect(isCallExpandChord(key({ ctrlKey: true }), {}, false)).toBe(false)
    expect(isCallExpandChord(key({ ctrlKey: true, shiftKey: true }), {}, false)).toBe(false)
  })

  it('AltGr (Ctrl+Alt on Windows) types a character, never Expand', () => {
    const altGr = key({ ctrlKey: true, altKey: true, getModifierState: (m: string) => m === 'AltGraph' })
    expect(isCallExpandChord(altGr, {}, false)).toBe(false)
  })

  it('follows a rebind', () => {
    const kb = { [CALL_EXPAND_HOTKEY]: 'Meta+Shift+Enter' }
    expect(isCallExpandChord(key({ key: 'Enter', code: 'Enter', metaKey: true, shiftKey: true }), kb, true)).toBe(true)
    expect(isCallExpandChord(key({ ctrlKey: true, metaKey: true }), kb, true)).toBe(false)
  })
})

describe("the call's window", () => {
  it('is known by its hash (dev) or its label (production)', () => {
    expect(isCallWindow('#call', null)).toBe(true)
    expect(isCallWindow('', 'window-call')).toBe(true)
    expect(isCallWindow('#ticket=t1', 'window-ticket-t1')).toBe(false)
    expect(isCallWindow('', 'main')).toBe(false)
  })

  it('keeps its frame per client, whole px; junk reads as nothing', () => {
    expect(readWindowFrame()).toBeNull()
    writeWindowFrame({ x: 10.4, y: 20.6, width: 800.2, height: 500 })
    expect(readWindowFrame()).toEqual({ x: 10, y: 21, width: 800, height: 500 })
    writeWindowFrame({ x: 0, y: 0, width: -1, height: 500 })
    expect(readWindowFrame()).toEqual({ x: 10, y: 21, width: 800, height: 500 })
    localStorage.setItem(CALL_WINDOW_FRAME_KEY, '{"x":1}')
    expect(readWindowFrame()).toBeNull()
  })

  it("counts the call's time from the earliest second any window saw it connect", () => {
    noteCallConnected('c1', 5_000)
    // A window that adopted it later doesn't move the clock forward.
    noteCallConnected('c1', 9_000)
    expect(JSON.parse(localStorage.getItem(CALL_CLOCK_KEY) as string)).toEqual({ callId: 'c1', at: 5_000 })
    expect(callStartedAt('c1', 9_000)).toBe(5_000)
    // Another call has its own clock.
    expect(callStartedAt('c2', 9_000)).toBe(9_000)
    noteCallConnected('c2', 7_000)
    expect(callStartedAt('c2', 9_000)).toBe(7_000)
    expect(callStartedAt('c2', null)).toBeNull()
  })
})
