// @vitest-environment jsdom
// Fail loud: placement order, guest→dock scaling, the mixed-version layout
// guard, per-window split widths, and the Design mode setting.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost: vi.fn(async () => ({ ok: true })) }))
vi.mock('@/lib/is-web', () => ({ isWebClient: vi.fn(() => false) }))
vi.mock('@/lib/window-chrome', () => ({ getWindowLabel: () => 'main' }))

import { daemonCliPost } from '@/lib/daemon-cli'
import { isWebClient } from '@/lib/is-web'
import type { ServerScope } from '@/kessel/server-scope'
import { placePopover, scaleGuestRect, POPOVER_GAP } from './placement'
import { designDataFromSerializedBrowser, isSerializedDesign, serializeDesignItem } from './design-serialize'
import { DEFAULT_DESIGN_SPLIT, designSplitKey, readDesignSplit, writeDesignSplit } from './design-split-sizes'
import { isDesignModeEnabled, setDesignMode } from './design-mode-setting'

const C = { x: 0, y: 0, width: 1000, height: 800 }
const POP = { width: 300, height: 150 }

describe('placePopover', () => {
  it('bottom first', () => {
    const p = placePopover({ x: 100, y: 100, width: 50, height: 20 }, POP, C)
    expect(p).toEqual({ side: 'bottom', x: 100, y: 120 + POPOVER_GAP, fallback: false })
  })
  it('top when the bottom is short', () => {
    const p = placePopover({ x: 100, y: 700, width: 50, height: 20 }, POP, C)
    expect(p.side).toBe('top')
    expect(p.y).toBe(700 - POPOVER_GAP - 150)
  })
  it('right when the element fills the height', () => {
    const p = placePopover({ x: 100, y: 50, width: 200, height: 700 }, POP, C)
    expect(p).toEqual({ side: 'right', x: 300 + POPOVER_GAP, y: 50, fallback: false })
  })
  it('left when only the left fits', () => {
    const p = placePopover({ x: 600, y: 50, width: 390, height: 700 }, POP, C)
    expect(p).toEqual({ side: 'left', x: 600 - POPOVER_GAP - 300, y: 50, fallback: false })
  })
  it('clamps inside the container on the chosen side', () => {
    const p = placePopover({ x: 900, y: 100, width: 80, height: 20 }, POP, C)
    expect(p.side).toBe('bottom')
    expect(p.x).toBe(700)
  })
  it('falls back to the roomiest side, clamped, when nothing fits', () => {
    const p = placePopover({ x: 10, y: 10, width: 980, height: 780 }, POP, C)
    expect(p.fallback).toBe(true)
    expect(p.x).toBeGreaterThanOrEqual(0)
    expect(p.x + POP.width).toBeLessThanOrEqual(1000)
    expect(p.y + POP.height).toBeLessThanOrEqual(800)
  })
})

describe('scaleGuestRect', () => {
  it('maps guest css px onto the dock box', () => {
    const r = scaleGuestRect({ x: 100, y: 50, width: 200, height: 40 }, { width: 1000, height: 500 }, { x: 400, y: 72, width: 500, height: 250 })
    expect(r).toEqual({ x: 450, y: 97, width: 100, height: 20 })
  })
  it('a zero guest viewport does not divide by zero', () => {
    const r = scaleGuestRect({ x: 1, y: 2, width: 3, height: 4 }, { width: 0, height: 0 }, { x: 10, y: 10, width: 5, height: 5 })
    expect(r).toEqual({ x: 11, y: 12, width: 3, height: 4 })
  })
})

describe('design-serialize (D-A4)', () => {
  const saved = serializeDesignItem('i1', { url: 'http://localhost:5173/', title: 'Home', icon: 'data:image/png;base64,x', navSeq: 4 })

  it('saves as a Browser item with a design marker and no navSeq', () => {
    expect(saved).toEqual({
      id: 'i1',
      type: 'browser',
      url: 'http://localhost:5173/',
      title: 'Home',
      icon: 'data:image/png;base64,x',
      design: { v: 1 },
    })
    expect(JSON.parse(JSON.stringify(saved))).toEqual(saved)
  })

  it('a new reader restores Design', () => {
    const roundTrip = JSON.parse(JSON.stringify(saved)) as unknown
    expect(isSerializedDesign(roundTrip)).toBe(true)
    expect(designDataFromSerializedBrowser(roundTrip, 7)).toEqual({
      url: 'http://localhost:5173/',
      title: 'Home',
      icon: 'data:image/png;base64,x',
      navSeq: 7,
    })
  })

  it('an old reader sees a Browser item at the same url', () => {
    // The pre-Design restore branch reads only these fields of a browser item.
    const roundTrip = JSON.parse(JSON.stringify(saved)) as { type: string; url?: string; title?: string; icon?: string }
    expect(roundTrip.type).toBe('browser')
    expect({ url: roundTrip.url, title: roundTrip.title, icon: roundTrip.icon }).toEqual({
      url: 'http://localhost:5173/',
      title: 'Home',
      icon: 'data:image/png;base64,x',
    })
  })

  it('a plain Browser item (or an old re-save) is not Design', () => {
    expect(designDataFromSerializedBrowser({ id: 'b', type: 'browser', url: 'https://x/' })).toBeNull()
    expect(designDataFromSerializedBrowser({ id: 'b', type: 'browser', url: 'https://x/', design: true })).toBeNull()
    expect(designDataFromSerializedBrowser({ id: 'f', type: 'file-viewer', design: { v: 1 } })).toBeNull()
  })
})

describe('design-split-sizes (D-A8)', () => {
  beforeEach(() => {
    localStorage.clear()
    sessionStorage.clear()
    vi.mocked(isWebClient).mockReturnValue(false)
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('defaults to 35/65', () => {
    expect(readDesignSplit('room-a', 'main')).toEqual([35, 65])
    expect(readDesignSplit(null, 'main')).toEqual([...DEFAULT_DESIGN_SPLIT])
  })

  it('keeps one width per room per window label', () => {
    writeDesignSplit('room-a', [40, 60], 'main')
    writeDesignSplit('room-b', [20, 80], 'main')
    writeDesignSplit('room-a', [50, 50], 'focus-x')
    expect(readDesignSplit('room-a', 'main')).toEqual([40, 60])
    expect(readDesignSplit('room-b', 'main')).toEqual([20, 80])
    expect(readDesignSplit('room-a', 'focus-x')).toEqual([50, 50])
    expect(JSON.parse(localStorage.getItem(designSplitKey('main')) ?? 'null')).toEqual({
      'room-a': [40, 60],
      'room-b': [20, 80],
    })
  })

  it('the web client uses sessionStorage', () => {
    vi.mocked(isWebClient).mockReturnValue(true)
    writeDesignSplit('room-a', [30, 70], 'main')
    expect(sessionStorage.getItem(designSplitKey('main'))).not.toBeNull()
    expect(localStorage.getItem(designSplitKey('main'))).toBeNull()
  })

  it('ignores malformed input and stored garbage', () => {
    writeDesignSplit('room-a', [0, 100], 'main')
    writeDesignSplit('room-a', [10, 20, 70], 'main')
    expect(localStorage.getItem(designSplitKey('main'))).toBeNull()
    localStorage.setItem(designSplitKey('main'), '{not json')
    expect(readDesignSplit('room-a', 'main')).toEqual([35, 65])
    localStorage.setItem(designSplitKey('main'), JSON.stringify({ 'room-a': ['x', 1] }))
    expect(readDesignSplit('room-a', 'main')).toEqual([35, 65])
  })

  it('storage that throws falls back to the default', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('denied')
    })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota')
    })
    expect(() => writeDesignSplit('room-a', [40, 60], 'main')).not.toThrow()
    expect(readDesignSplit('room-a', 'main')).toEqual([35, 65])
  })
})

describe('design mode setting', () => {
  it('missing means on; 0 / false mean off', () => {
    expect(isDesignModeEnabled(undefined)).toBe(true)
    expect(isDesignModeEnabled({})).toBe(true)
    expect(isDesignModeEnabled({ designModeEnabled: 1 })).toBe(true)
    expect(isDesignModeEnabled({ designModeEnabled: 0 })).toBe(false)
    expect(isDesignModeEnabled({ designModeEnabled: false })).toBe(false)
    expect(isDesignModeEnabled({ designModeEnabled: true })).toBe(true)
  })

  it('writes design_mode_enabled through workspace/set', async () => {
    const scope = { id: 'primary', isPrimary: true } as unknown as ServerScope
    await setDesignMode(scope, '/w/docs', false)
    await setDesignMode(scope, '/w/docs', true)
    expect(vi.mocked(daemonCliPost).mock.calls).toEqual([
      [scope, 'workspace/set', { project: '/w/docs', fields: { design_mode_enabled: '0' } }],
      [scope, 'workspace/set', { project: '/w/docs', fields: { design_mode_enabled: '1' } }],
    ])
  })
})
