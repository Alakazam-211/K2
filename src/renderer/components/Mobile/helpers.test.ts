// Mobile tab pure pieces (prd-mobile-dev-v1 S4), fail loud:
//  - the mixed-version marker (M-A7): a Mobile item saves as an empty
//    Browser item plus `mobile: { v: 1 }`;
//  - the split width under its own prefix, apart from Design's;
//  - gestures and keys → device input (§2.4, M-A9);
//  - the route adapter's parses refuse a body they don't understand;
//  - the stub stream the tab is built against.

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
import {
  isSerializedMobile,
  mobileDataFromSerializedBrowser,
  serializeMobileItem,
} from './mobile-serialize'
import { DEFAULT_MOBILE_SPLIT, mobileSplitKey, readMobileSplit, writeMobileSplit } from './mobile-split-sizes'
import { canvasPointToDevice, deviceRectToCanvas, gestureFor, keyToSimInput, type KeyLike } from './sim-input'
import { firstUsableLane, parseSimDevices, parseSimStatus } from './simApi'
import { createStubSimStream, defaultFps, simStreamUrl, type SimStreamHandlers } from './sim-stream'

beforeEach(() => mem.clear())

describe('mobile marker (M-A7)', () => {
  it('saves as an empty Browser item with the marker and restores as Mobile', () => {
    const si = serializeMobileItem('i1', { deviceType: 'pixel-9' })
    expect(si).toEqual({ id: 'i1', type: 'browser', url: '', title: 'Mobile', mobile: { v: 1, deviceType: 'pixel-9' } })
    const disk = JSON.parse(JSON.stringify(si)) as unknown
    expect(isSerializedMobile(disk)).toBe(true)
    expect(mobileDataFromSerializedBrowser(disk)).toEqual({ deviceType: 'pixel-9' })
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

describe('gestures → device input (§2.4, §5)', () => {
  const box = { left: 100, top: 50, width: 206, height: 457.5 }
  const meta = { width: 412, height: 915 }

  it('maps canvas px to device dp, clamped to the screen', () => {
    expect(canvasPointToDevice(100, 50, box, meta)).toEqual({ x: 0, y: 0 })
    expect(canvasPointToDevice(203, 278.75, box, meta)).toEqual({ x: 206, y: 458 })
    expect(canvasPointToDevice(9999, -5, box, meta)).toEqual({ x: 412, y: 0 })
    expect(deviceRectToCanvas({ x: 206, y: 0, width: 100, height: 50 }, box, meta)).toEqual({
      left: 203,
      top: 50,
      width: 50,
      height: 25,
    })
  })

  it('a short still press is a tap; a drag or a long press is a swipe', () => {
    const tap = gestureFor({ clientX: 150, clientY: 100, at: 0 }, { clientX: 153, clientY: 102, at: 120 }, box, meta)
    expect(tap).toEqual({ action: 'tap', x: 100, y: 100 })
    const swipe = gestureFor({ clientX: 200, clientY: 400, at: 0 }, { clientX: 200, clientY: 150, at: 300 }, box, meta)
    expect(swipe).toEqual({ action: 'swipe', from: { x: 200, y: 700 }, to: { x: 200, y: 200 }, durationMs: 300 })
    const longPress = gestureFor({ clientX: 150, clientY: 100, at: 0 }, { clientX: 150, clientY: 100, at: 900 }, box, meta)
    expect(longPress).toEqual({ action: 'swipe', from: { x: 100, y: 100 }, to: { x: 100, y: 100 }, durationMs: 900 })
  })

  it('keys: ordinary keys go to the device; ⌘ chords stay with K2 except ⌘⇧H and ⌘D; Esc leaves', () => {
    const k = (key: string, mods: Partial<KeyLike> = {}): ReturnType<typeof keyToSimInput> =>
      keyToSimInput({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods })
    expect(k('a')).toEqual({ send: { action: 'type', text: 'a' } })
    expect(k('A', { shiftKey: true })).toEqual({ send: { action: 'type', text: 'A' } })
    expect(k('é')).toEqual({ send: { action: 'type', text: 'é' } })
    expect(k(' ')).toEqual({ send: { action: 'type', text: ' ' } })
    expect(k('Enter')).toEqual({ send: { action: 'key', key: 'Enter' } })
    expect(k('Backspace')).toEqual({ send: { action: 'key', key: 'Backspace' } })
    expect(k('ArrowLeft')).toEqual({ send: { action: 'key', key: 'ArrowLeft' } })
    expect(k('H', { metaKey: true, shiftKey: true })).toEqual({ send: { action: 'button', button: 'home' } })
    expect(k('d', { metaKey: true })).toEqual({ send: { action: 'button', button: 'menu' } })
    expect(k('Escape')).toBe('blur')
    for (const chord of ['w', 't', 'k', 'l', ',', '1']) expect(k(chord, { metaKey: true })).toBe('pass')
    expect(k('M', { metaKey: true, shiftKey: true })).toBe('pass')
    expect(k('D', { metaKey: true, shiftKey: true })).toBe('pass')
    expect(k('c', { ctrlKey: true })).toBe('pass')
    expect(k('Shift', { shiftKey: true })).toBe('pass')
    expect(k('F5')).toBe('pass')
    expect(k('a', { isComposing: true })).toBe('pass')
  })
})

describe('route adapter parses (fail loud)', () => {
  const session = {
    id: 's1',
    lane: 'android',
    device: 'Pixel 9',
    state: 'running',
    orientation: 'portrait',
    screen: { width: 412, height: 915 },
    app: 'com.acme.app',
    control: { heldBy: 'person', by: 'rosson' },
  }

  it('reads a status with lanes, a session and the caller’s control', () => {
    const st = parseSimStatus({
      lanes: [
        { lane: 'ios', usable: false, reason: 'Not a Mac.' },
        { lane: 'android', usable: true },
      ],
      session,
      canControl: false,
    })
    expect(st.canControl).toBe(false)
    expect(st.session).toEqual({ ...session, control: { heldBy: 'person', by: 'rosson' } })
    expect(firstUsableLane(st.lanes)?.lane).toBe('android')
    expect(parseSimStatus({ lanes: [], session: null, canControl: true }).session).toBeNull()
  })

  it('refuses bodies it does not understand', () => {
    expect(() => parseSimStatus({ lanes: [], session: null })).toThrow(/unexpected sim status/)
    expect(() => parseSimStatus({ lanes: [{ lane: 'windows', usable: true }], session: null, canControl: true })).toThrow(/sim lane/)
    expect(() => parseSimStatus({ lanes: [], session: { ...session, state: 'dancing' }, canControl: true })).toThrow(/sim session/)
    expect(() => parseSimStatus({ lanes: [], session: { ...session, screen: { width: 0, height: 1 } }, canControl: true })).toThrow(/sim screen/)
    expect(() => parseSimDevices({ devices: [{ id: 'x', name: 'X' }] })).toThrow(/sim device/)
    expect(parseSimDevices({ devices: [{ id: 'pixel-9', name: 'Pixel 9', lane: 'android' }] })).toEqual([
      { id: 'pixel-9', name: 'Pixel 9', lane: 'android' },
    ])
  })
})

describe('stream adapter', () => {
  it('builds the socket URL with the sim id, the fps cap and the token', () => {
    expect(simStreamUrl('ws://127.0.0.1:9', 's1', 'tok', 8)).toBe('ws://127.0.0.1:9/cli/sim/stream?sim=s1&fps=8&token=tok')
    expect(simStreamUrl('wss://box.k2.dev', 's 2', '', 30)).toBe('wss://box.k2.dev/cli/sim/stream?sim=s+2&fps=30')
    expect(defaultFps(false)).toBe(30)
    expect(defaultFps(true)).toBe(8)
  })

  it('the stub opens, sends meta, pushes frames, records input and rotates', async () => {
    const stub = createStubSimStream()
    const seen: string[] = []
    const handlers: SimStreamHandlers = {
      onFrame: (f) => seen.push(`frame:${f.seq}:${f.source.kind}`),
      onMeta: (m) => seen.push(`meta:${m.width}x${m.height}:${m.orientation}`),
      onControl: (c) => seen.push(`control:${c.heldBy}`),
      onState: (s) => seen.push(`state:${s}`),
    }
    const conn = stub.connector({ scope: {} as never, simId: 's1', fps: 30, handlers })
    await Promise.resolve()
    stub.pushFrame()
    conn.send({ action: 'tap', x: 1, y: 2 })
    conn.send({ action: 'rotate', direction: 'right' })
    conn.ack(1)
    stub.pushControl({ heldBy: 'takeover' })
    expect(stub.openCount()).toBe(1)
    conn.close()
    expect(stub.openCount()).toBe(0)
    stub.pushFrame()
    expect(seen).toEqual([
      'state:connecting',
      'state:open',
      'meta:412x915:portrait',
      'frame:1:paint',
      'meta:915x412:landscape',
      'control:takeover',
    ])
    expect(stub.inputs).toEqual([{ action: 'tap', x: 1, y: 2 }, { action: 'rotate', direction: 'right' }])
    expect(stub.acks).toEqual([1])
  })
})
