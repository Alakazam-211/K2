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
import { createZenBridge, isEmptyZenGardenPage, registerZenVerb, ZenBridgeError, type ZenBridgeHost } from './zen-bridge'
import { installZenChordListener, isZenChordNonMac } from './zen-shortcut'
import {
  BUILTIN_BLANK_PAGE,
  BUILTIN_TEXTING_PAGE,
  parseZenGet,
  ZenPageParseError,
  zenControlPlacement,
  zenErrorBannerText,
  zenMenuLabel,
  zenMenuRequiredControls,
  zenWidgetInBand,
  zenWidgetSlot,
  type ZenResolvedPage,
} from './zen-page'
import { zenPageForView, zenRailOrientation } from './zen-rail-views'
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
  function host(
    page: ZenResolvedPage = BUILTIN_TEXTING_PAGE,
  ): ZenBridgeHost & { exited: number; switched: string[]; created: string[]; made: unknown[]; templated: unknown[] } {
    const h = {
      exited: 0,
      switched: [] as string[],
      created: [] as string[],
      made: [] as unknown[],
      templated: [] as unknown[],
      gardens: () => [
        { id: 'g-a', name: 'Garden 1', index: 1 },
        { id: 'g-b', name: 'Notes', index: 2 },
      ],
      currentGardenId: () => 'g-a',
      switchGarden: (id: string) => void h.switched.push(id),
      createGarden: async (name: string, template?: string, opts?: unknown) => {
        h.created.push(name)
        h.made.push([name, template, opts])
        return { id: 'g-c', name, index: 3 }
      },
      useGardenTemplate: async (id: string, template: string, force: boolean) => {
        h.templated.push([id, template, force])
        return { changed: true }
      },
      renameGarden: async () => undefined,
      deleteGarden: async () => undefined,
      homes: () => [{ id: 'a', name: 'A' }],
      exit: () => void (h.exited += 1),
      controls: { bind: () => () => undefined, bindings: () => [], wiringFailure: () => null, menuWiringFailure: () => null, dispose: () => undefined },
      page: () => page,
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
    // Rosson 2026-10-04: + New Garden says what the Garden starts as.
    await controls.gardens.create('Desk', 'texting')
    await controls.gardens.create('Ideas', 'blank', { ask: true })
    expect(h.made.slice(1)).toEqual([
      ['Desk', 'texting', {}],
      ['Ideas', 'blank', { ask: true }],
    ])
    expect(codeOf(() => controls.call('gardens.create', 'X', 'dashboard'))).toBe('unknown_verb')
  })

  it('gardens.empty / useTemplate need gardens:template (the empty-Garden widget only) and act on the Garden on screen', async () => {
    const blank = host(BUILTIN_BLANK_PAGE)
    const controls = createZenBridge(blank, { id: 'template-controls', caps: ['agents:add', 'gardens:manage'] })
    expect(codeOf(() => controls.call('gardens.useTemplate', 'texting'))).toBe('cap_not_granted')
    expect(codeOf(() => controls.call('gardens.empty'))).toBe('cap_not_granted')
    const caps = ['agents:read', 'thread:read', 'thread:post', 'gardens:template']
    const widget = createZenBridge(blank, { id: 'garden-empty', caps })
    expect(widget.call('gardens.empty')).toBe(true)
    await expect(widget.call('gardens.useTemplate', 'texting') as Promise<unknown>).resolves.toEqual({ changed: true })
    await widget.call('gardens.useTemplate', 'texting', { force: true })
    // Never another Garden: the id is the one on screen, whatever is passed.
    await widget.call('gardens.useTemplate', 'blank', { force: 'yes', garden: 'g-b' })
    expect(blank.templated).toEqual([
      ['g-a', 'texting', false],
      ['g-a', 'texting', true],
      ['g-a', 'blank', false],
    ])
    expect(codeOf(() => widget.call('gardens.useTemplate', 'dashboard'))).toBe('unknown_verb')
    expect(codeOf(() => widget.call('gardens.useTemplate'))).toBe('unknown_verb')
    // Not empty: Garden 1's page, or a blank Garden with widgets of its own.
    expect(createZenBridge(host(BUILTIN_TEXTING_PAGE), { id: 'garden-empty', caps }).call('gardens.empty')).toBe(false)
    const own = { ...BUILTIN_BLANK_PAGE, widgets: [...BUILTIN_BLANK_PAGE.widgets, { ...BUILTIN_TEXTING_PAGE.widgets[0], column: 0 }] }
    expect(isEmptyZenGardenPage(own)).toBe(false)
    expect(isEmptyZenGardenPage(BUILTIN_BLANK_PAGE)).toBe(true)
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
      { name: 'basic', builtin: true, user: false, summary: 'clean', active: false },
      { name: 'paper', builtin: true, user: true, summary: 'warm', active: true },
      { name: 'mine', builtin: false, user: true, summary: '', active: false },
    ]
    const p = parseZenGet({ version: 'v', page, theme: { name: 'paper', scope: 'garden', tokens: {} }, themes })
    expect(p.activeTheme).toBe('paper')
    expect(p.themeScope).toBe('garden')
    expect(p.themes).toEqual([
      { name: 'basic', label: 'Basic', builtin: true, user: false },
      { name: 'paper', label: 'Paper', builtin: true, user: true },
      { name: 'mine', label: 'Mine', builtin: false, user: true },
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
      ['garden-empty', 'garden-empty', ['agents:read', 'thread:read', 'thread:post', 'gardens:template']],
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

  it('a band widget keeps its slot (`top`); a column widget has none; a slot this client does not know is a column', () => {
    const p = parseZenGet({
      version: 'v',
      page: {
        template: 'k2.texting@1',
        layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
        widgets: [
          { id: 'agents', kind: 'agents', slot: 'column', column: 0, props: {}, caps: [] },
          { id: 'nav-rail', kind: 'nav-rail', slot: 'top', props: { orientation: 'row' }, caps: ['app:navigate'] },
          { id: 'later', kind: 'nav-rail', slot: 'side', column: 1, props: {}, caps: [] },
        ],
        controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
      },
    })
    expect(p.widgets.map((w) => [w.id, w.slot, w.column, zenWidgetSlot(w), zenWidgetInBand(w)])).toEqual([
      ['agents', undefined, 0, 'column', false],
      ['nav-rail', 'top', 0, 'top', true],
      ['later', undefined, 1, 'column', false],
    ])
    // `bottom` is a band this client draws now (prd-zen-freeform-chrome S2).
    expect(zenWidgetSlot({ slot: 'bottom' })).toBe('bottom')
    expect(zenRailOrientation(p.widgets[1])).toBe('row')
    expect(zenRailOrientation({ slot: 'top', props: {} })).toBe('row')
    expect(zenRailOrientation({ props: {} })).toBe('column')
    expect(zenRailOrientation({ props: { orientation: 'row' } })).toBe('row')
    // The rail views keep a band rail in its band.
    const tickets = zenPageForView(p, 'tickets')
    expect(tickets.widgets.map((w) => [w.kind, zenWidgetSlot(w)])).toEqual([
      ['nav-rail', 'top'],
      ['nav-rail', 'column'],
      ['tickets-view', 'column'],
    ])
  })

  it('FC-T6: a zen-chrome-v1 answer parses chrome, bands, edges and menus; chrome stays out of widgets (FC28, FC29)', () => {
    // The daemon's `bottom-bar` answer, with a bottom-band rail.
    const p = parseZenGet({
      version: 'v',
      page: {
        template: 'k2.texting@1',
        layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
        widgets: [
          { id: 'agents', kind: 'agents', slot: 'column', column: 0, props: {}, caps: [] },
          { id: 'conversation', kind: 'conversation', slot: 'column', column: 1, props: {}, caps: [] },
          { id: 'nav-rail', kind: 'nav-rail', slot: 'bottom', align: 'end', props: { orientation: 'row' }, caps: ['app:navigate'] },
        ],
        controls: [
          { kind: 'garden-switcher', placement: 'bottom-start' },
          { kind: 'drag-region', placement: 'bands' },
          { kind: 'zen-toggle', placement: 'bottom-end' },
          { kind: 'add-agent', placement: 'widget-bottom-left', widget: 'agents' },
        ],
        chrome: {
          from: 'garden',
          items: [
            { id: 'garden-switcher', kind: 'garden-switcher', slot: 'bottom', align: 'start', props: {}, caps: ['gardens:manage'] },
            { id: 'zen-toggle', kind: 'zen-toggle', slot: 'bottom', align: 'end', props: {}, caps: [] },
          ],
        },
        bands: { top: null, bottom: { start: ['garden-switcher'], center: [], end: ['zen-toggle', 'nav-rail'] } },
        edges: [],
        menus: {},
      },
    })
    expect(p.controls).toEqual(['garden-switcher', 'drag-region', 'zen-toggle', 'add-agent'])
    expect(p.widgets.map((w) => w.kind)).toEqual(['agents', 'conversation', 'nav-rail'])
    // `bottom` is a band this renderer draws; the rail keeps its `align`.
    expect(p.widgets.map((w) => [zenWidgetSlot(w), w.align])).toEqual([
      ['column', undefined],
      ['column', undefined],
      ['bottom', 'end'],
    ])
    expect(p.placement).toEqual({
      from: 'garden',
      items: [
        { id: 'garden-switcher', kind: 'garden-switcher', slot: 'bottom', align: 'start', props: {}, caps: ['gardens:manage'] },
        { id: 'zen-toggle', kind: 'zen-toggle', slot: 'bottom', align: 'end', props: {}, caps: [] },
      ],
      bands: { top: null, bottom: { start: ['garden-switcher'], center: [], end: ['zen-toggle', 'nav-rail'] } },
      edges: [],
      menus: {},
    })
  })

  it('FC-T6: menus, column edges and ids the page lacks; a chrome kind sent as a widget is never a column box', () => {
    const p = parseZenGet({
      version: 'v',
      page: {
        template: 'k2.texting@1',
        layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
        widgets: [
          { id: 'agents', kind: 'agents', column: 0, props: {}, caps: [] },
          { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: [] },
          // A daemon bug: chrome in widgets. Never drawn as a placeholder.
          { id: 'zen-toggle', kind: 'zen-toggle', column: 1, props: {}, caps: [] },
        ],
        controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
        chrome: {
          from: 'garden',
          items: [
            { id: 'more', kind: 'menu', slot: 'column', column: 1, edge: 'bottom', align: 'end', props: { icon: 'dots', label: 'More' }, caps: [] },
            { id: 'garden-switcher', kind: 'garden-switcher', slot: 'menu', menu: 'more', props: {}, caps: ['gardens:manage'] },
            { id: 'zen-toggle', kind: 'zen-toggle', slot: 'menu', menu: 'more', props: {}, caps: [] },
          ],
        },
        bands: { top: null, bottom: null },
        edges: [{ column: 1, edge: 'bottom', start: [], center: [], end: ['more', 'ghost'] }],
        menus: { more: ['garden-switcher', 'zen-toggle', 'ghost'] },
      },
    })
    expect(p.widgets.map((w) => w.id)).toEqual(['agents', 'conversation'])
    expect(p.placement.edges).toEqual([{ column: 1, edge: 'bottom', start: [], center: [], end: ['more'] }])
    expect(p.placement.menus).toEqual({ more: ['garden-switcher', 'zen-toggle'] })
    expect(zenControlPlacement(p.placement, 'zen-toggle')?.menu?.id).toBe('more')
    expect(zenMenuRequiredControls(p.placement, 'more')).toEqual(['garden-switcher', 'zen-toggle'])
    expect(zenMenuLabel(p.placement.items[0])).toBe('More')
    expect(zenMenuLabel({ props: {} })).toBe('More')
    expect(zenMenuLabel({ props: { label: '  Zen  ' } })).toBe('Zen')
  })

  it('FC-T6 / FC32: an older daemon (no page.chrome) gets the template’s chrome; a top-band rail sits after the switcher', () => {
    const p = parseZenGet({
      version: 'v',
      page: {
        template: 'k2.texting@1',
        layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
        widgets: [
          { id: 'agents', kind: 'agents', column: 0, props: {}, caps: [] },
          { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: [] },
          { id: 'nav-rail', kind: 'nav-rail', slot: 'top', props: { orientation: 'row' }, caps: ['app:navigate'] },
        ],
        controls: ['garden-switcher', 'drag-region', 'zen-toggle', 'add-agent'],
      },
    })
    expect(p.placement.from).toBe('template')
    expect(p.placement.items.map((i) => [i.kind, i.slot, i.align])).toEqual([
      ['garden-switcher', 'top', 'start'],
      ['usage', 'top', 'end'],
      ['theme-picker', 'top', 'end'],
      ['zen-toggle', 'top', 'end'],
    ])
    expect(p.placement.bands).toEqual({
      top: { start: ['garden-switcher', 'nav-rail'], center: [], end: ['usage', 'theme-picker', 'zen-toggle'] },
      bottom: null,
    })
    expect(p.placement.edges).toEqual([])
    // A row rail in a column (no edge) runs across that column's top edge.
    const row = parseZenGet({
      page: {
        template: 'k2.texting@1',
        layout: { kind: 'columns', split: [50, 50] },
        widgets: [{ id: 'nav', kind: 'nav-rail', column: 1, props: { orientation: 'row' }, caps: [] }],
        controls: [],
      },
    })
    expect(row.placement.edges).toEqual([{ column: 1, edge: 'top', start: ['nav'], center: [], end: [] }])
    // Safe mode's page and both built-ins carry the template's chrome.
    for (const b of [BUILTIN_TEXTING_PAGE, BUILTIN_BLANK_PAGE]) {
      expect(b.placement.bands.top).toEqual({ start: ['garden-switcher'], center: [], end: ['usage', 'theme-picker', 'zen-toggle'] })
      expect(b.placement.bands.bottom).toBeNull()
    }
  })

  it('FC48: a one-column view moves column-edge items to column 0, same edge and align, in order', () => {
    const page: ZenResolvedPage = {
      ...BUILTIN_TEXTING_PAGE,
      placement: {
        from: 'garden',
        items: [
          { id: 'garden-switcher', kind: 'garden-switcher', slot: 'column', column: 0, edge: 'bottom', align: 'start', props: {}, caps: [] },
          { id: 'zen-toggle', kind: 'zen-toggle', slot: 'column', column: 1, edge: 'bottom', align: 'end', props: {}, caps: [] },
          { id: 'theme-picker', kind: 'theme-picker', slot: 'column', column: 1, edge: 'top', align: 'end', props: {}, caps: [] },
        ],
        bands: { top: null, bottom: null },
        edges: [
          { column: 0, edge: 'bottom', start: ['garden-switcher'], center: [], end: [] },
          { column: 1, edge: 'top', start: [], center: [], end: ['theme-picker'] },
          { column: 1, edge: 'bottom', start: [], center: [], end: ['zen-toggle'] },
        ],
        menus: {},
      },
    }
    const tickets = zenPageForView(page, 'tickets')
    expect(tickets.layout.split).toEqual([100])
    expect(tickets.placement.edges).toEqual([
      { column: 0, edge: 'top', start: [], center: [], end: ['theme-picker'] },
      { column: 0, edge: 'bottom', start: ['garden-switcher'], center: [], end: ['zen-toggle'] },
    ])
    expect(tickets.placement.items.map((i) => [i.id, i.column])).toEqual([
      ['garden-switcher', 0],
      ['zen-toggle', 0],
      ['theme-picker', 0],
    ])
    // The Agents view keeps the page's placement as is.
    expect(zenPageForView(page, 'agents').placement).toBe(page.placement)
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
