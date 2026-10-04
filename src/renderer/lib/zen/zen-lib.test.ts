// @vitest-environment jsdom
//
// prd-zen-mode-v1 S4 — the pure pieces: the per-Home store (Z5), the shown
// rule, the bridge's cap check (Z33/Z34), the chord (Z30/Z55), page
// parsing (Z10/Z13), the stoplight safe area (Z23) and where Zen exists
// (Z2, Q5).

import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))

import {
  attachZenHomesStorageSync,
  createZenHomesStore,
  parseZenHomesDoc,
  ZEN_HOMES_STORAGE_KEY,
  type KeyValueStorage,
} from './zen-homes'
import { computeZenShown, zenSafeCauseText } from './zen-view'
import { createZenBridge, registerZenVerb, ZenBridgeError, type ZenBridgeHost } from './zen-bridge'
import { installZenChordListener, isZenChordNonMac } from './zen-shortcut'
import { BUILTIN_TEXTING_PAGE, parseZenGet, ZenPageParseError, zenErrorBannerText } from './zen-page'
import { macStoplightArea } from './zen-chrome'
import { isZenWindow, zenSupportedOn, ZEN_WINDOWS_ENABLED } from './zen-platform'
import { TRAFFIC_LIGHT_SPACER_BASE_PX } from '@/lib/desktop-chrome'

function memKv(): KeyValueStorage & { map: Map<string, string> } {
  const map = new Map<string, string>()
  return { map, getItem: (k) => map.get(k) ?? null, setItem: (k, v) => void map.set(k, v) }
}

describe('k2.zen.homes.v1 (Z5)', () => {
  it('is per Home and persists; a second store (another window) reads it', () => {
    const kv = memKv()
    const a = createZenHomesStore(kv)
    a.getState().setOn('work', true)
    a.getState().setOn('home', true)
    a.getState().setOn('home', false)
    expect(a.getState().on).toEqual({ work: true })
    expect(JSON.parse(kv.map.get(ZEN_HOMES_STORAGE_KEY) ?? 'null')).toEqual({ version: 1, on: { work: true } })
    const b = createZenHomesStore(kv)
    expect(b.getState().on).toEqual({ work: true })
  })

  it('follows another window through the storage event, and a clear', () => {
    const kv = memKv()
    const store = createZenHomesStore(kv)
    const target = new EventTarget()
    const off = attachZenHomesStorageSync(store, target, kv)
    const ev = (key: string | null, newValue: string | null): Event =>
      Object.assign(new Event('storage'), { key, newValue })
    target.dispatchEvent(ev(ZEN_HOMES_STORAGE_KEY, JSON.stringify({ version: 1, on: { x: true } })))
    expect(store.getState().on).toEqual({ x: true })
    target.dispatchEvent(ev('k2.homes.v1', '{}'))
    expect(store.getState().on).toEqual({ x: true })
    target.dispatchEvent(ev(null, null))
    expect(store.getState().on).toEqual({})
    off()
  })

  it('parses only version 1 and drops non-true values', () => {
    expect(parseZenHomesDoc(null)).toBeNull()
    expect(parseZenHomesDoc('junk')).toBeNull()
    expect(parseZenHomesDoc(JSON.stringify({ version: 2, on: {} }))).toBeNull()
    expect(parseZenHomesDoc(JSON.stringify({ version: 1, on: { a: true, b: 1, c: 'yes' } }))).toEqual({
      version: 1,
      on: { a: true },
    })
  })

  it('a storage that throws falls back to memory', () => {
    const bad: KeyValueStorage = {
      getItem: () => {
        throw new Error('locked')
      },
      setItem: () => {
        throw new Error('locked')
      },
    }
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const store = createZenHomesStore(bad)
    store.getState().setOn('a', true)
    expect(store.getState().on).toEqual({ a: true })
    warn.mockRestore()
  })
})

describe('shown (Z4/Z5)', () => {
  it('only on Home, Settings closed, the Home on, Zen available', () => {
    const base = { available: true, page: 'home', settingsOpen: false, homeOn: true }
    expect(computeZenShown(base)).toBe(true)
    expect(computeZenShown({ ...base, page: 'agents' })).toBe(false)
    expect(computeZenShown({ ...base, settingsOpen: true })).toBe(false)
    expect(computeZenShown({ ...base, homeOn: false })).toBe(false)
    expect(computeZenShown({ ...base, available: false })).toBe(false)
  })

  it('names each cause', () => {
    expect(zenSafeCauseText({ kind: 'control', control: 'home-switcher', problem: 'invisible' })).toBe(
      'The Home switcher isn’t visible.',
    )
    expect(zenSafeCauseText({ kind: 'crash', message: 'x' })).toBe('The page crashed: x')
    expect(zenSafeCauseText({ kind: 'unreachable', message: 'x' })).toBe('Can’t reach K2 on this computer.')
  })
})

describe('desktop only (Z2, Q5)', () => {
  it('never on web; Windows waits for the G-Win smoke; never in Focus or ticket windows', () => {
    expect(zenSupportedOn(true, 'mac')).toBe(false)
    expect(zenSupportedOn(true, 'linux')).toBe(false)
    expect(zenSupportedOn(false, 'mac')).toBe(true)
    expect(zenSupportedOn(false, 'linux')).toBe(true)
    expect(zenSupportedOn(false, 'windows')).toBe(ZEN_WINDOWS_ENABLED)
    expect(ZEN_WINDOWS_ENABLED).toBe(false)
    expect(isZenWindow('', 'main')).toBe(true)
    expect(isZenWindow('', 'window-1234')).toBe(true)
    expect(isZenWindow('', 'focus-abc')).toBe(false)
    expect(isZenWindow('', 'window-ticket-42')).toBe(false)
    expect(isZenWindow('#focus=abc', null)).toBe(false)
    expect(isZenWindow('#ticket=42', null)).toBe(false)
  })
})

describe('the bridge (Z33/Z34)', () => {
  function host(): ZenBridgeHost & { exited: number; selected: string[] } {
    const h = {
      exited: 0,
      selected: [] as string[],
      homes: () => [{ id: 'a', name: 'A' }],
      selectedHomeId: () => 'a',
      selectHome: (id: string) => void h.selected.push(id),
      exit: () => void (h.exited += 1),
      controls: { bind: () => () => undefined, bindings: () => [], wiringFailure: () => null, dispose: () => undefined },
      page: () => BUILTIN_TEXTING_PAGE,
    }
    return h
  }

  it('an undeclared verb throws cap_not_granted', () => {
    const b = createZenBridge(host(), { id: 'w', caps: ['agents:read'] })
    let err: unknown = null
    try {
      b.call('thread.post', 'x', 'hi')
    } catch (e) {
      err = e
    }
    expect(err).toBeInstanceOf(ZenBridgeError)
    expect((err as ZenBridgeError).code).toBe('cap_not_granted')
  })

  it('a declared data verb with no implementation yet throws verb_unavailable; a registered one runs with the widget id', () => {
    const b = createZenBridge(host(), { id: 'w1', caps: ['agents:read'] })
    expect(() => b.call('agents.list')).toThrow(/verb_unavailable/)
    const off = registerZenVerb('agents.list', (ctx) => [ctx.widgetId])
    expect(b.call('agents.list')).toEqual(['w1'])
    off()
    expect(() => b.call('agents.list')).toThrow(/verb_unavailable/)
    expect(() => registerZenVerb('zen.exit', () => null)).toThrow(/built in/)
  })

  it('every page has the no-cap verbs', () => {
    const h = host()
    const b = createZenBridge(h, { id: 'controls', caps: [] })
    expect(b.homes.list()).toEqual([{ id: 'a', name: 'A' }])
    b.homes.select('a')
    b.zen.exit()
    expect(h.selected).toEqual(['a'])
    expect(h.exited).toBe(1)
    expect(b.theme.get()).toEqual({ theme: null, chrome: null, motion: null })
    expect(() => b.call('nope' as never)).toThrow(/unknown_verb/)
  })
})

describe('the chord (Z30/Z55)', () => {
  const ev = (p: Partial<{ code: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean; altGraph: boolean }>) => ({
    code: p.code ?? 'KeyZ',
    ctrlKey: p.ctrlKey ?? true,
    altKey: p.altKey ?? true,
    metaKey: p.metaKey ?? false,
    shiftKey: p.shiftKey ?? false,
    getModifierState: (k: string) => k === 'AltGraph' && Boolean(p.altGraph),
  })

  it('Ctrl+Alt+Z only; AltGr, Meta, Shift or another key are not it', () => {
    expect(isZenChordNonMac(ev({}))).toBe(true)
    expect(isZenChordNonMac(ev({ altGraph: true }))).toBe(false)
    expect(isZenChordNonMac(ev({ metaKey: true }))).toBe(false)
    expect(isZenChordNonMac(ev({ shiftKey: true }))).toBe(false)
    expect(isZenChordNonMac(ev({ code: 'KeyY' }))).toBe(false)
    expect(isZenChordNonMac(ev({ altKey: false }))).toBe(false)
  })

  it('macOS installs no webview listener (the native accelerator is the one owner); Linux and Windows capture', () => {
    const added: Array<[string, boolean]> = []
    const target = {
      addEventListener: (t: string, _f: unknown, capture?: boolean) => void added.push([t, Boolean(capture)]),
      removeEventListener: () => undefined,
    } as unknown as Window
    expect(installZenChordListener('mac', target, () => undefined)).toBeNull()
    expect(installZenChordListener('other', target, () => undefined)).toBeNull()
    expect(added).toEqual([])
    expect(installZenChordListener('linux', target, () => undefined)).not.toBeNull()
    expect(installZenChordListener('windows', target, () => undefined)).not.toBeNull()
    expect(added).toEqual([
      ['keydown', true],
      ['keydown', true],
    ])
  })
})

describe('the resolved page (Z10, Z13)', () => {
  it('parses the contract shape', () => {
    const p = parseZenGet({
      ok: false,
      schema: 1,
      version: 'abc',
      page: {
        template: 'k2.texting@1',
        layout: { kind: 'columns', split: [40, 60], min_widths: [200, 300] },
        widgets: [{ id: 'a', type: 'agents', col: 0, caps: ['agents:read', 7] }],
        controls: ['zen-toggle', { kind: 'home-switcher' }, 'drag-region'],
      },
      errors: [{ file: 'zen.toml', line: 7, col: 3, message: "unknown key 'acent'" }],
    })
    expect(p.version).toBe('abc')
    expect(p.layout).toEqual({ kind: 'columns', split: [40, 60], minWidths: [200, 300] })
    expect(p.widgets).toEqual([
      { id: 'a', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' },
    ])
    expect(p.controls).toEqual(['zen-toggle', 'home-switcher', 'drag-region'])
    expect(zenErrorBannerText(p.errors[0])).toBe("zen.toml line 7: unknown key 'acent'. Showing your last good version.")
  })

  it('falls back to the template for a missing layout or widgets, but never for controls', () => {
    const p = parseZenGet({ version: 'v', page: { template: 'k2.texting@1' } })
    expect(p.layout).toEqual(BUILTIN_TEXTING_PAGE.layout)
    expect(p.widgets.map((w) => w.kind)).toEqual(['agents', 'conversation'])
    expect(p.controls).toEqual([])
  })

  it('throws for no page, a non-object, or another schema', () => {
    expect(() => parseZenGet('x')).toThrow(ZenPageParseError)
    expect(() => parseZenGet({ ok: false, error: 'zen_local_only' })).toThrow(/zen_local_only/)
    expect(() => parseZenGet({ schema: 2, page: {} })).toThrow(/schema 1/)
  })
})

describe('the stoplight safe area (Z23)', () => {
  it('rides the zoom-aware --k2-stoplight-spacer and matches its numbers at 100%', () => {
    const a = macStoplightArea(0, 1)
    expect(a.vars['--zen-stoplight-safe-left']).toBe('calc(26px + var(--k2-stoplight-spacer))')
    // 12 (bar pad) + spacer (57 at 100%) + 14 (gap): the same left edge the
    // top bar's first control has.
    expect(12 + TRAFFIC_LIGHT_SPACER_BASE_PX + 14).toBe(83)
    expect(a.rect.width).toBeGreaterThanOrEqual(69)
    expect(a.vars['--zen-stoplight-safe-top']).toBe('31px')
    // Zoomed in, the native lights take fewer CSS px.
    expect(macStoplightArea(0, 2).rect.width).toBeLessThan(a.rect.width)
  })
})
