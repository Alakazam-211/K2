// @vitest-environment jsdom
//
// The call's video as this client sizes it (Rosson 2026-11-10): the tile's
// width clamps and keeps 16:9, grows from its inward edges, is remembered per
// client; the full-screen chord; the call's window's frame and clock.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { HOTKEYS, getDefaultKeybindings } from '@shared/hotkeys'
import {
  CALL_CLOCK_KEY,
  CALL_EXPAND_HOTKEY,
  CALL_TILE_DEFAULT_WIDTH,
  CALL_TILE_MAX_WIDTH,
  CALL_TILE_MIN_WIDTH,
  CALL_TILE_WIDTH_KEY,
  CALL_WINDOW_FRAME_KEY,
  __resetTileWidthForTests,
  callStartedAt,
  clampTileWidth,
  inwardEdges,
  isCallExpandChord,
  isCallWindow,
  maxTileWidth,
  noteCallConnected,
  readTileWidth,
  readWindowFrame,
  resizedWidth,
  writeTileWidth,
  writeWindowFrame,
} from './call-view'

const BIG = { width: 3000, height: 3000, below: 40 }

beforeEach(() => {
  localStorage.clear()
  __resetTileWidthForTests()
})
afterEach(() => localStorage.clear())

describe('the tile width', () => {
  it('clamps to the minimum and the maximum', () => {
    expect(clampTileWidth(50, BIG)).toBe(CALL_TILE_MIN_WIDTH)
    expect(clampTileWidth(5000, BIG)).toBe(CALL_TILE_MAX_WIDTH)
    expect(clampTileWidth(333.4, BIG)).toBe(333)
    expect(clampTileWidth(Number.NaN, BIG)).toBe(CALL_TILE_DEFAULT_WIDTH)
  })

  it("never outgrows the room: the free area's width, or its height through 16:9 less the strip", () => {
    expect(maxTileWidth({ width: 500, height: 3000, below: 40 })).toBe(500)
    // 400 high, 40 for the strip: 360 × 16/9 = 640.
    expect(maxTileWidth({ width: 3000, height: 400, below: 40 })).toBe(640)
    expect(clampTileWidth(900, { width: 3000, height: 400, below: 40 })).toBe(640)
  })

  it('a room smaller than the minimum still gets the minimum (the strip needs it)', () => {
    expect(clampTileWidth(300, { width: 120, height: 80, below: 40 })).toBe(CALL_TILE_MIN_WIDTH)
  })

  it('grows from the edges away from its corner', () => {
    expect(inwardEdges('bottom-right')).toEqual({ x: -1, y: -1 })
    expect(inwardEdges('top-left')).toEqual({ x: 1, y: 1 })
    expect(inwardEdges('bottom-left')).toEqual({ x: 1, y: -1 })
    expect(inwardEdges('top-right')).toEqual({ x: -1, y: 1 })
    const br = inwardEdges('bottom-right')
    // Dragging the left edge left grows it; right shrinks it.
    expect(resizedWidth(240, -100, 0, 'x', br)).toBe(340)
    expect(resizedWidth(240, 40, 0, 'x', br)).toBe(200)
    // The top edge: up grows it, through the aspect.
    expect(resizedWidth(240, 0, -90, 'y', br)).toBe(400)
    // An edge ignores the other axis.
    expect(resizedWidth(240, -100, 500, 'x', br)).toBe(340)
    // The corner follows the axis that moved further.
    expect(resizedWidth(240, -10, -90, 'corner', br)).toBe(400)
    expect(resizedWidth(240, -200, -9, 'corner', br)).toBe(440)
  })

  it('is remembered per client, and a bad saved value is the default', () => {
    expect(readTileWidth()).toBe(CALL_TILE_DEFAULT_WIDTH)
    writeTileWidth(412.6)
    expect(localStorage.getItem(CALL_TILE_WIDTH_KEY)).toBe('413')
    __resetTileWidthForTests()
    expect(readTileWidth()).toBe(413)
    localStorage.setItem(CALL_TILE_WIDTH_KEY, 'wide')
    expect(readTileWidth()).toBe(CALL_TILE_DEFAULT_WIDTH)
  })
})

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
