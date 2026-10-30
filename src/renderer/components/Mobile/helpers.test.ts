// Mobile tab pure pieces (prd-mobile-dev-v1 S4), fail loud:
//  - the mixed-version marker (M-A7): a Mobile item saves as an empty
//    Browser item plus `mobile: { v: 1 }`;
//  - the split width under its own prefix, apart from Design's;
//  - gestures and keys → stream input in points (§2.4, M-A9,
//    k2-core::sim::stream::ViewerMsg);
//  - who may control (remote Members watch only).
// The route and wire adapters are in contract.test.ts.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { designSplitKey, readDesignSplit, writeDesignSplit } from '@/components/Design/design-split-sizes'
import { isSerializedDesign } from '@/components/Design/design-serialize'
import { isSerializedMobile, mobileDataFromSerializedBrowser, serializeMobileItem } from './mobile-serialize'
import { DEFAULT_MOBILE_SPLIT, mobileSplitKey, readMobileSplit, writeMobileSplit } from './mobile-split-sizes'
import {
  canvasPointToDevice,
  deviceRectToCanvas,
  gestureFor,
  liveTouchEnd,
  liveTouchHold,
  liveTouchMove,
  liveTouchStart,
  nextOrientation,
} from './sim-input'
import { simControlFor } from './useSimCanControl'

beforeEach(() => mem.clear())

describe('mobile marker (M-A7)', () => {
  it('saves as an empty Browser item with the marker and restores as Mobile', () => {
    const si = serializeMobileItem('i1', { deviceType: 'pixel_9' })
    expect(si).toEqual({ id: 'i1', type: 'browser', url: '', title: 'Mobile', mobile: { v: 1, deviceType: 'pixel_9' } })
    const disk = JSON.parse(JSON.stringify(si)) as unknown
    expect(isSerializedMobile(disk)).toBe(true)
    expect(mobileDataFromSerializedBrowser(disk)).toEqual({ deviceType: 'pixel_9' })
    // Not mistaken for a Design item.
    expect(isSerializedDesign(disk)).toBe(false)
  })

  it('a plain Browser item, a malformed marker or a non-browser type is not Mobile', () => {
    expect(mobileDataFromSerializedBrowser({ id: 'b', type: 'browser', url: 'https://x' })).toBeNull()
    expect(mobileDataFromSerializedBrowser({ id: 'b', type: 'browser', url: '', mobile: true })).toBeNull()
    expect(mobileDataFromSerializedBrowser({ id: 'b', type: 'browser', url: '', mobile: { v: '1' } })).toBeNull()
    expect(mobileDataFromSerializedBrowser({ id: 'b', type: 'file-viewer', mobile: { v: 1 } })).toBeNull()
    expect(mobileDataFromSerializedBrowser(null)).toBeNull()
    // No device yet → empty data.
    expect(mobileDataFromSerializedBrowser(serializeMobileItem('i2', {}))).toEqual({})
  })
})

describe('mobile split (k2.windowChrome.mobile.)', () => {
  it('keeps its own width per window per room, apart from Design', () => {
    expect(mobileSplitKey('main')).toBe('k2.windowChrome.mobile.main')
    expect(readMobileSplit('room-a', 'main')).toEqual([...DEFAULT_MOBILE_SPLIT])
    writeMobileSplit('room-a', [30, 70], 'main')
    writeDesignSplit('room-a', [60, 40], 'main')
    expect(readMobileSplit('room-a', 'main')).toEqual([30, 70])
    expect(readDesignSplit('room-a', 'main')).toEqual([60, 40])
    expect(JSON.parse(mem.get(mobileSplitKey('main'))!)).toEqual({ 'room-a': [30, 70] })
    expect(JSON.parse(mem.get(designSplitKey('main'))!)).toEqual({ 'room-a': [60, 40] })
    // Another window keeps its own.
    expect(readMobileSplit('room-a', 'focus-x')).toEqual([...DEFAULT_MOBILE_SPLIT])
    // Malformed input is not saved.
    writeMobileSplit('room-a', [0, 100], 'main')
    expect(readMobileSplit('room-a', 'main')).toEqual([30, 70])
  })
})

describe('gestures → device input (§2.4, §5; stream::ViewerMsg)', () => {
  const box = { left: 100, top: 50, width: 206, height: 457.5 }
  const screen = { widthPt: 412, heightPt: 915 }

  it('maps canvas px to device points (cx / canvas_w * wpt), clamped to the screen', () => {
    expect(canvasPointToDevice(100, 50, box, screen)).toEqual({ x: 0, y: 0 })
    expect(canvasPointToDevice(203, 278.75, box, screen)).toEqual({ x: 206, y: 457.5 })
    expect(canvasPointToDevice(9999, -5, box, screen)).toEqual({ x: 412, y: 0 })
    expect(deviceRectToCanvas({ x: 206, y: 0, width: 100, height: 50 }, box, screen)).toEqual({
      left: 203,
      top: 50,
      width: 50,
      height: 25,
    })
  })

  it('a short still press is a tap; a drag or a long press is a swipe', () => {
    const tap = gestureFor({ clientX: 150, clientY: 100, at: 0 }, { clientX: 153, clientY: 102, at: 120 }, box, screen)
    expect(tap).toEqual({ t: 'tap', x: 100, y: 100 })
    const swipe = gestureFor({ clientX: 200, clientY: 400, at: 0 }, { clientX: 200, clientY: 150, at: 300 }, box, screen)
    expect(swipe).toEqual({ t: 'swipe', x1: 200, y1: 700, x2: 200, y2: 200, durationMs: 300 })
    const longPress = gestureFor({ clientX: 150, clientY: 100, at: 0 }, { clientX: 150, clientY: 100, at: 900 }, box, screen)
    expect(longPress).toEqual({ t: 'swipe', x1: 100, y1: 100, x2: 100, y2: 100, durationMs: 900 })
  })

  it('live touch: down waits past the slop, then down + move…, up; a clean tap stays a tap', () => {
    const t0 = liveTouchStart({ clientX: 150, clientY: 100, at: 0 })
    const still = liveTouchMove(t0, { clientX: 153, clientY: 102, at: 10 }, box, screen)
    expect(still.send).toEqual([])
    expect(liveTouchEnd(still.touch, { clientX: 153, clientY: 102, at: 80 }, box, screen)).toEqual([{ t: 'tap', x: 100, y: 100 }])
    const drag = liveTouchMove(t0, { clientX: 150, clientY: 130, at: 30 }, box, screen)
    expect(drag.send).toEqual([
      { t: 'touch', phase: 'down', x: 100, y: 100 },
      { t: 'touch', phase: 'move', x: 100, y: 160 },
    ])
    const more = liveTouchMove(drag.touch, { clientX: 150, clientY: 160, at: 50 }, box, screen)
    expect(more.send).toEqual([{ t: 'touch', phase: 'move', x: 100, y: 220 }])
    expect(liveTouchEnd(more.touch, { clientX: 150, clientY: 200, at: 90 }, box, screen)).toEqual([
      { t: 'touch', phase: 'up', x: 100, y: 300 },
    ])
  })

  it('live touch: a long press starts in place, once', () => {
    const t0 = liveTouchStart({ clientX: 150, clientY: 100, at: 0 })
    const held = liveTouchHold(t0, box, screen)
    expect(held.send).toEqual([{ t: 'touch', phase: 'down', x: 100, y: 100 }])
    expect(liveTouchHold(held.touch, box, screen).send).toEqual([])
    expect(liveTouchEnd(held.touch, { clientX: 150, clientY: 100, at: 900 }, box, screen)).toEqual([
      { t: 'touch', phase: 'up', x: 100, y: 100 },
    ])
  })

  it('rotate turns a quarter from what the frames show', () => {
    expect(nextOrientation({ widthPt: 412, heightPt: 915 })).toBe('landscape-right')
    expect(nextOrientation({ widthPt: 915, heightPt: 412 })).toBe('portrait')
  })
})

describe('who may control (Rosson 2026-10-27: remote Members watch)', () => {
  it('the daemon’s answer wins; else local yes, remote Owner/Admin yes, Member or unknown no', () => {
    const c = (daemonSays: boolean | null, isRemote: boolean, role: 'owner' | 'admin' | 'member' | null, roomMayWrite = true): boolean =>
      simControlFor({ daemonSays, isRemote, role, roomMayWrite })
    expect(c(false, false, 'owner')).toBe(false)
    expect(c(true, true, 'member')).toBe(true)
    expect(c(null, false, null)).toBe(true)
    expect(c(null, true, 'owner')).toBe(true)
    expect(c(null, true, 'admin')).toBe(true)
    expect(c(null, true, 'member')).toBe(false)
    expect(c(null, true, null)).toBe(false)
    // A Home room whose server's sim routes are not on the allowlist.
    expect(c(true, true, 'owner', false)).toBe(false)
  })
})
