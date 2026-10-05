// @vitest-environment jsdom
//
// prd-zen-mode-v1 S4 and prd-zen-gardens-v1 — the pure pieces: the
// per-window store (G1/G21, TG3.1), the shown rule (G2), the Agents
// widget's Home picks (G27), the Garden list parse (G13), the bridge's cap
// check (Z33/Z34, G29, TG4.3), the chord (Z30/Z55), page parsing
// (Z10/Z13, G12, G38), the stoplight safe area (Z23) and where Zen exists
// (Z2, Q5).

import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))

import { createZenWindowStore, parseZenWindowDoc, zenWindowKey, type KeyValueStorage } from './zen-window'
import {
  attachZenGardenHomesStorageSync,
  createZenGardenHomesStore,
  ZEN_GARDEN_HOMES_KEY,
} from './zen-garden-homes'
import { parseZenGardenNew, parseZenGardens, windowGardenOf } from './zen-gardens'
import { computeZenShown, zenSafeCauseText } from './zen-view'
import { createZenBridge, registerZenVerb, ZenBridgeError, type ZenBridgeHost } from './zen-bridge'
import { installZenChordListener, isZenChordNonMac } from './zen-shortcut'
import { BUILTIN_TEXTING_PAGE, parseZenGet, ZenPageParseError, zenErrorBannerText } from './zen-page'
import { macStoplightArea } from './zen-chrome'
import { isZenWindow, zenSupportedOn, ZEN_WINDOWS_ENABLED } from './zen-platform'
import { TRAFFIC_LIGHT_SPACER_BASE_PX } from '@/lib/desktop-chrome'
import { ZEN_STOPLIGHT_INSET_PX } from '@/lib/traffic-lights'

function memKv(): KeyValueStorage & { map: Map<string, string> } {
  const map = new Map<string, string>()
  return { map, getItem: (k) => map.get(k) ?? null, setItem: (k, v) => void map.set(k, v) }
}

describe('k2.zen.window.v1.<label> (G1, G21, TG3.1)', () => {
  it('is per window: on in one label leaves the other off; a relaunch (new store, same label) restores on and garden', () => {
    const kv = memKv()
    const main = createZenWindowStore(kv, 'main')
    const other = createZenWindowStore(kv, 'window-x')
    expect(main.getState().on).toBe(false)
    expect(other.getState().on).toBe(false)
    main.getState().setOn(true)
    main.getState().setGarden('g-1')
    expect(main.getState().on).toBe(true)
    expect(other.getState().on).toBe(false)
    expect(other.getState().garden).toBeNull()
    expect(JSON.parse(kv.map.get(zenWindowKey('main')) ?? 'null')).toEqual({ version: 1, on: true, garden: 'g-1', view: 'home' })
    expect(kv.map.has(zenWindowKey('window-x'))).toBe(false)
    // Relaunch: the same label comes back as it was.
    const again = createZenWindowStore(kv, 'main')
    expect(again.getState()).toMatchObject({ label: 'main', on: true, garden: 'g-1' })
    // A new window (a new label) starts outside Zen.
    expect(createZenWindowStore(kv, 'window-new').getState()).toMatchObject({ on: false, garden: null })
  })

  it('no sync between windows: another window saving its own key changes nothing here', () => {
    const kv = memKv()
    const a = createZenWindowStore(kv, 'main')
    const b = createZenWindowStore(kv, 'window-b')
    b.getState().setOn(true)
    expect(a.getState().on).toBe(false)
  })

  it('parses only version 1 with a boolean on; leaves junk alone until a change', () => {
    expect(parseZenWindowDoc(null)).toBeNull()
    expect(parseZenWindowDoc('junk')).toBeNull()
    expect(parseZenWindowDoc(JSON.stringify({ version: 2, on: true }))).toBeNull()
    expect(parseZenWindowDoc(JSON.stringify({ version: 1, on: 'yes' }))).toBeNull()
    expect(parseZenWindowDoc(JSON.stringify({ version: 1, on: true, garden: 7 }))).toEqual({ version: 1, on: true, garden: null })
    const kv = memKv()
    kv.map.set(zenWindowKey('main'), '{"version":9}')
    const store = createZenWindowStore(kv, 'main')
    expect(store.getState().on).toBe(false)
    expect(kv.map.get(zenWindowKey('main'))).toBe('{"version":9}')
  })

  it('remembers the nav rail’s view per window (Rosson 2026-10-04); an older or unknown view is My Home', () => {
    expect(parseZenWindowDoc(JSON.stringify({ version: 1, on: true, garden: 'g-1', view: 'tickets' }))).toEqual({
      version: 1,
      on: true,
      garden: 'g-1',
      view: 'tickets',
    })
    expect(parseZenWindowDoc(JSON.stringify({ version: 1, on: true, garden: 'g-1', view: 'settings' }))).toEqual({
      version: 1,
      on: true,
      garden: 'g-1',
    })
    const kv = memKv()
    const a = createZenWindowStore(kv, 'main')
    expect(a.getState().view).toBe('home')
    a.getState().setView('agents')
    expect(() => a.getState().setView('settings' as never)).toThrow(/unknown rail view/)
    // A relaunch (new store, same label) comes back to it; another window doesn't.
    expect(createZenWindowStore(kv, 'main').getState().view).toBe('agents')
    expect(createZenWindowStore(kv, 'window-b').getState().view).toBe('home')
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
    const store = createZenWindowStore(bad, 'main')
    store.getState().setOn(true)
    store.getState().setGarden('g-2')
    expect(store.getState()).toMatchObject({ on: true, garden: 'g-2' })
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })
})

describe('the Agents widget’s Home picks (G27)', () => {
  it('per Garden and widget, saved, and seen by another window through the storage event', () => {
    const kv = memKv()
    const a = createZenGardenHomesStore(kv)
    a.getState().setPick('g-1/agents', 'h2')
    expect(JSON.parse(kv.map.get(ZEN_GARDEN_HOMES_KEY) ?? 'null')).toEqual({ version: 1, picks: { 'g-1/agents': 'h2' } })
    // A remount (a new store) keeps it.
    expect(createZenGardenHomesStore(kv).getState().picks).toEqual({ 'g-1/agents': 'h2' })
    // Another window's store follows the storage event.
    const b = createZenGardenHomesStore(memKv())
    const target = new EventTarget()
    const off = attachZenGardenHomesStorageSync(b, target, kv)
    target.dispatchEvent(
      Object.assign(new Event('storage'), { key: ZEN_GARDEN_HOMES_KEY, newValue: kv.map.get(ZEN_GARDEN_HOMES_KEY) }),
    )
    expect(b.getState().picks).toEqual({ 'g-1/agents': 'h2' })
    off()
  })
})

describe('the Garden list (G13, G22)', () => {
  it('parses the gardens answer, and tells "not set up" from a list', () => {
    expect(parseZenGardens({ ok: true, setUp: false, gardens: [] })).toEqual({ setUp: false, gardens: [] })
    const l = parseZenGardens({
      ok: true,
      setUp: true,
      gardens: [
        { id: 'g-3f9a12c0', name: 'Garden 1', index: 1, template: 'k2.texting@1', hasFile: true, seedHome: 'h1', createdAt: 'x' },
        { id: 'g-00000001', name: 'Notes', index: 2, template: 'k2.blank@1', hasFile: true },
        { id: 'g-00000001', name: 'dup' },
        { name: 'no id' },
      ],
    })
    expect(l.setUp).toBe(true)
    expect(l.gardens.map((g) => [g.id, g.name, g.index, g.template, g.seedHome])).toEqual([
      ['g-3f9a12c0', 'Garden 1', 1, 'k2.texting@1', 'h1'],
      ['g-00000001', 'Notes', 2, 'k2.blank@1', null],
    ])
    expect(() => parseZenGardens({ ok: false, error: 'zen_local_only' })).toThrow(/zen_local_only/)
    expect(() => parseZenGardens('x')).toThrow(/not an object/)
    expect(parseZenGardenNew({ ok: true, garden: { id: 'g-1', name: 'N' } }).id).toBe('g-1')
    expect(() => parseZenGardenNew({ ok: true })).toThrow(/missing/)
  })

  it('the window’s Garden is its pick when listed, else the first', () => {
    const gardens = parseZenGardens({ gardens: [{ id: 'a', name: 'A' }, { id: 'b', name: 'B' }] }).gardens
    expect(windowGardenOf(gardens, 'b')?.id).toBe('b')
    expect(windowGardenOf(gardens, 'gone')?.id).toBe('a')
    expect(windowGardenOf(gardens, null)?.id).toBe('a')
    expect(windowGardenOf([], 'a')).toBeNull()
  })
})

describe('shown (G2)', () => {
  it('this window’s switch on, Settings closed, Zen available; the page is not part of it', () => {
    const base = { available: true, on: true, settingsOpen: false }
    expect(computeZenShown(base)).toBe(true)
    expect(computeZenShown({ ...base, settingsOpen: true })).toBe(false)
    expect(computeZenShown({ ...base, on: false })).toBe(false)
    expect(computeZenShown({ ...base, available: false })).toBe(false)
  })

  it('names each cause', () => {
    expect(zenSafeCauseText({ kind: 'control', control: 'garden-switcher', problem: 'invisible' })).toBe(
      'The Garden switcher isn’t visible.',
    )
    expect(zenSafeCauseText({ kind: 'crash', message: 'x' })).toBe('The page crashed: x')
    expect(zenSafeCauseText({ kind: 'unreachable', message: 'x' })).toBe('Can’t reach K2 on this computer.')
    expect(zenSafeCauseText({ kind: 'outdated' })).toBe('K2 on this computer is older than this app. Update it to use Gardens.')
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

describe('the bridge (Z33/Z34, G29, TG4.3)', () => {
  function host(): ZenBridgeHost & { exited: number; switched: string[]; created: string[] } {
    const h = {
      exited: 0,
      switched: [] as string[],
      created: [] as string[],
      gardens: () => [
        { id: 'g-a', name: 'Garden 1', index: 1 },
        { id: 'g-b', name: 'Notes', index: 2 },
      ],
      currentGardenId: () => 'g-a',
      switchGarden: (id: string) => void h.switched.push(id),
      createGarden: async (name: string) => {
        h.created.push(name)
        return { id: 'g-c', name, index: 3 }
      },
      renameGarden: async () => undefined,
      deleteGarden: async () => undefined,
      homes: () => [{ id: 'a', name: 'A' }],
      exit: () => void (h.exited += 1),
      controls: { bind: () => () => undefined, bindings: () => [], wiringFailure: () => null, dispose: () => undefined },
      page: () => BUILTIN_TEXTING_PAGE,
    }
    return h
  }

  function codeOf(fn: () => unknown): string {
    try {
      fn()
    } catch (e) {
      if (e instanceof ZenBridgeError) return e.code
      throw e
    }
    throw new Error('the call did not throw')
  }

  it('an undeclared verb throws cap_not_granted', () => {
    const b = createZenBridge(host(), { id: 'w', caps: ['agents:read'] })
    expect(codeOf(() => b.call('thread.post', 'x', 'hi'))).toBe('cap_not_granted')
  })

  it('a declared data verb with no implementation yet throws verb_unavailable; a registered one runs with the widget, page and Garden', () => {
    const b = createZenBridge(host(), { id: 'w1', caps: ['agents:read'] })
    expect(() => b.call('agents.list')).toThrow(/verb_unavailable/)
    const off = registerZenVerb('agents.list', (ctx) => [ctx.widgetId, ctx.gardenId(), ctx.page().template])
    expect(b.call('agents.list')).toEqual(['w1', 'g-a', 'k2.texting@1'])
    off()
    expect(() => b.call('agents.list')).toThrow(/verb_unavailable/)
    expect(() => registerZenVerb('zen.exit', () => null)).toThrow(/built in/)
    expect(() => registerZenVerb('homes.list', () => null)).toThrow(/built in/)
  })

  it('gardens.list / current / switch work with no caps; every page has them', () => {
    const h = host()
    const b = createZenBridge(h, { id: 'controls', caps: [] })
    expect(b.gardens.list().map((g) => g.id)).toEqual(['g-a', 'g-b'])
    expect(b.gardens.current()).toEqual({ id: 'g-a', name: 'Garden 1', index: 1 })
    b.gardens.switch('g-b')
    expect(h.switched).toEqual(['g-b'])
    b.zen.exit()
    expect(h.exited).toBe(1)
    expect(b.theme.get()).toEqual({ theme: null, chrome: null, motion: null })
    expect(() => b.call('nope' as never)).toThrow(/unknown_verb/)
  })

  it('gardens.create / rename / delete need gardens:manage; the template controls get it, a widget does not', async () => {
    const h = host()
    const widget = createZenBridge(h, { id: 'agents', caps: ['agents:read', 'agents:add', 'presence:read'] })
    expect(codeOf(() => widget.call('gardens.create', 'X'))).toBe('cap_not_granted')
    expect(codeOf(() => widget.call('gardens.rename', 'g-a', 'X'))).toBe('cap_not_granted')
    expect(codeOf(() => widget.call('gardens.delete', 'g-a'))).toBe('cap_not_granted')
    expect(h.created).toEqual([])
    const controls = createZenBridge(h, { id: 'template-controls', caps: ['agents:add', 'gardens:manage'] })
    await expect(controls.gardens.create('Mornings')).resolves.toEqual({ id: 'g-c', name: 'Mornings', index: 3 })
    expect(h.created).toEqual(['Mornings'])
  })

  it('homes.select is gone (unknown_verb); homes.list needs agents:read', () => {
    const b = createZenBridge(host(), { id: 'controls', caps: [] })
    expect(codeOf(() => b.call('homes.select' as never, 'a'))).toBe('unknown_verb')
    expect(codeOf(() => b.homes.list())).toBe('cap_not_granted')
    const reader = createZenBridge(host(), { id: 'agents', caps: ['agents:read'] })
    expect(reader.homes.list()).toEqual([{ id: 'a', name: 'A' }])
  })

  it('compose.draft needs thread:post; agents.home / setHome / local need agents:read', () => {
    const none = createZenBridge(host(), { id: 'w', caps: ['thread:read'] })
    expect(codeOf(() => none.call('compose.draft', 'a::local', 'hi'))).toBe('cap_not_granted')
    expect(codeOf(() => none.call('agents.home'))).toBe('cap_not_granted')
    expect(codeOf(() => none.call('agents.setHome', 'a'))).toBe('cap_not_granted')
    expect(codeOf(() => none.call('agents.local'))).toBe('cap_not_granted')
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
        controls: ['zen-toggle', { kind: 'garden-switcher' }, 'drag-region'],
      },
      errors: [{ file: 'zen.toml', line: 7, col: 3, message: "unknown key 'acent'" }],
    })
    expect(p.version).toBe('abc')
    expect(p.layout).toEqual({ kind: 'columns', split: [40, 60], minWidths: [200, 300] })
    expect(p.widgets).toEqual([
      { id: 'a', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' },
    ])
    expect(p.controls).toEqual(['zen-toggle', 'garden-switcher', 'drag-region'])
    expect(zenErrorBannerText(p.errors[0])).toBe("zen.toml line 7: unknown key 'acent'. Showing your last good version.")
  })

  it('falls back to the template for a missing layout or widgets, but never for controls', () => {
    const p = parseZenGet({ version: 'v', page: { template: 'k2.texting@1' } })
    expect(p.layout).toEqual(BUILTIN_TEXTING_PAGE.layout)
    expect(p.widgets.map((w) => w.kind)).toEqual(['agents', 'conversation', 'nav-rail'])
    expect(p.controls).toEqual([])
    expect(p.activeTheme).toBeNull()
    expect(p.themeScope).toBe('global')
    expect(p.themes).toEqual([])
  })

  it('reads the daemon’s active theme, its scope and the theme list', () => {
    const page = { template: 'k2.texting@1' }
    const themes = [
      { name: 'default', builtin: true, user: false, summary: 'clean', active: false },
      { name: 'paper', builtin: true, user: true, summary: 'warm', active: true },
      { name: 'mine', builtin: false, user: true, summary: '', active: false },
    ]
    const p = parseZenGet({ version: 'v', page, theme: { name: 'paper', scope: 'garden', tokens: {} }, themes })
    expect(p.activeTheme).toBe('paper')
    expect(p.themeScope).toBe('garden')
    expect(p.themes).toEqual([
      { name: 'default', builtin: true, user: false },
      { name: 'paper', builtin: true, user: true },
      { name: 'mine', builtin: false, user: true },
    ])
    // No `theme.name`: the list's `active` flag names it; an odd scope is global.
    const q = parseZenGet({ version: 'v', page, theme: { scope: 'everywhere' }, themes })
    expect(q.activeTheme).toBe('paper')
    expect(q.themeScope).toBe('global')
  })

  it('reads which Garden the page is (G12), and a blank template falls back to the blank page (G11)', () => {
    const p = parseZenGet({ version: 'v', page: { template: 'k2.blank@1' }, garden: { id: 'g-1', name: 'Notes', index: 2 } })
    expect(p.garden).toEqual({ id: 'g-1', name: 'Notes', index: 2 })
    expect(p.widgets.map((w) => [w.id, w.kind, w.caps])).toEqual([
      ['garden-empty', 'garden-empty', ['agents:read', 'thread:read', 'thread:post']],
    ])
    expect(p.layout.split).toEqual([100])
    expect(parseZenGet({ version: 'v', page: {} }).garden).toBeNull()
  })

  it('G38: a Garden’s own layout as column tables, and its built-in widgets with props', () => {
    const p = parseZenGet({
      version: 'v',
      page: {
        template: 'k2.blank@1',
        layout: { kind: 'columns', columns: [{ size: 30, 'min-width': 220 }, { size: 70 }] },
        widgets: [
          { id: 'work', kind: 'agents', column: 0, props: { home: 'Work', 'home-picker': true }, caps: ['agents:read'] },
          { id: 'talk', kind: 'conversation', column: 1, props: { agents: 'work' }, caps: ['thread:read'] },
        ],
        controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
      },
    })
    expect(p.layout).toEqual({ kind: 'columns', split: [30, 70], minWidths: [220, 0] })
    expect(p.widgets.map((w) => [w.id, w.column, w.props])).toEqual([
      ['work', 0, { home: 'Work', 'home-picker': true }],
      ['talk', 1, { agents: 'work' }],
    ])
  })

  it('throws for no page, a non-object, or another schema', () => {
    expect(() => parseZenGet('x')).toThrow(ZenPageParseError)
    expect(() => parseZenGet({ ok: false, error: 'zen_local_only' })).toThrow(/zen_local_only/)
    expect(() => parseZenGet({ schema: 2, page: {} })).toThrow(/schema 1/)
  })
})

describe('the stoplight safe area (Z23)', () => {
  it('matches the Styles spacer numbers at 100% and follows the zoom and Zen offset', () => {
    const a = macStoplightArea([0, 0], 1)
    // The top bar's first control starts at 12 (bar pad) + spacer (57 at
    // 100%) + 14 (gap) = 83. Zen moves the lights 8 right and 8 down
    // (Rosson 2026-10-04), so the page starts 8 further right and below.
    expect(12 + TRAFFIC_LIGHT_SPACER_BASE_PX + 14 + ZEN_STOPLIGHT_INSET_PX).toBe(91)
    expect(a.vars['--zen-stoplight-safe-left']).toBe('91px')
    expect(a.rect.width).toBe(77)
    expect(a.vars['--zen-stoplight-safe-top']).toBe('39px')
    // Zoomed in, the native lights take fewer CSS px.
    expect(macStoplightArea([0, 0], 2).rect.width).toBeLessThan(a.rect.width)
    // Zen's stoplight-offset moves the area right and down.
    const moved = macStoplightArea([10, 6], 1)
    expect(moved.rect.width).toBe(87)
    expect(moved.vars['--zen-stoplight-safe-left']).toBe('101px')
    expect(moved.vars['--zen-stoplight-safe-top']).toBe('45px')
  })
})
