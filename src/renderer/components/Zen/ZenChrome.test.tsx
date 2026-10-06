// @vitest-environment jsdom
//
// prd-zen-freeform-chrome S2 / S3 (§6 "S2/S3 renderer") — K2's controls
// drawn from data (`page.chrome`, `page.bands`, `page.edges`,
// `page.menus`), the `menu` container, and the placement-aware required-
// controls check, through the real ZenHost, Zen root, page, chrome
// registry, bridge, control registry and safe mode. Only the edges are
// faked: the daemon (`daemon-cli`), the app socket, Tauri, and layout
// (jsdom has none: the check reads an injected geometry, and the row
// overflow reads `clientWidth` / `offsetWidth` set per element).
//
// Asserted (fail loudly):
//   - each guide example's page answer draws its layout: rail-top,
//     quiet-top, bottom-bar, menu-both, texting-chrome, column-corner, and
//     none of them trips safe mode;
//   - an older daemon (no `page.chrome`) gets the template's top band;
//   - with no top band, the title strip stays (bound for drag, empty);
//   - a menu holding both required controls passes the check while
//     closed; a hidden required control (direct, or its menu's button)
//     fails it with the menu named; a button off the keyboard is
//     `no-keyboard`; a menu that doesn't bind within 1 s is `not-wired`;
//   - an open menu over a control isn't "covering" it (FC-T9);
//   - a crowded band hides usage, then theme, then truncates the
//     switcher; the required controls never hide (FC-T10);
//   - dropdowns in the bottom band open upward (FC-T11);
//   - keyboard: Tab to the menu button, Enter, arrows, Exit Zen Mode;
//     sub-panels and Esc return focus (FC-T12);
//   - Shift on entry draws the template's chrome, not the Garden's (FC-T13);
//   - rail views keep the chrome; column-edge items move to column 0 in a
//     one-column view; no false alarm on a view switch or a menu
//     open / close (FC-T14, FC26).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; route: string; data: unknown }>,
  page: null as null | Record<string, unknown>,
  gardens: [
    { id: 'g-1', name: 'Garden 1', template: 'k2.texting@1' },
    { id: 'g-2', name: 'Garden 2', template: 'k2.blank@1' },
  ],
  invokes: [] as Array<{ cmd: string; args: unknown }>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: unknown) => {
    h.invokes.push({ cmd, args })
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: 'main',
    startDragging: async () => undefined,
    isMaximized: async () => false,
    maximize: async () => undefined,
    unmaximize: async () => undefined,
    minimize: async () => undefined,
    close: async () => undefined,
    listen: async () => () => undefined,
  }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', route, data: params })
    if (route === 'zen/gardens') {
      return { ok: true, setUp: true, gardens: h.gardens.map((g, i) => ({ ...g, index: i + 1, hasFile: true })) }
    }
    if (route === 'zen/get') {
      if (!h.page) throw new Error('no page set for this test')
      return h.page
    }
    if (route === 'usage/subscriptions') return { harnesses: [] }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', route, data: body })
    if (route === 'zen/theme/set') return { ok: true }
    throw new Error(`unexpected POST ${route}`)
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...mod,
    onZenChanged: () => () => undefined,
    subscribeToActiveState: () => () => undefined,
  }
})

import { act, useState } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { registerZenChrome, type ZenChromeProps } from './zen-registry'
import { useZenBind } from './ZenTemplateControls'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __resetZenGardensForTests } from '@/lib/zen/zen-gardens'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { __setZenGeometryForTests, runZenControlChecksNow } from '@/lib/zen/zen-monitor'
import { ZEN_WIRING_DEADLINE_MS, type ZenGeometry, type ZenRect } from '@/lib/zen/zen-controls'
import { enterZen, useZenViewStore } from '@/lib/zen/zen-view'
import { useZenOverlayStore } from '@/lib/zen/zen-theme-switch'
import { ZEN_USAGE_CSS } from './ZenUsageTool'
import { ZenHost } from './ZenHost'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

// ── Page answers, as the daemon sends them (docs/zen-contract.md) ────────

type Groups = { start: string[]; center: string[]; end: string[] }
const g = (start: string[], center: string[], end: string[]): Groups => ({ start, center, end })

const AGENTS = { id: 'agents', kind: 'agents', slot: 'column', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' }
const CONVERSATION = { id: 'conversation', kind: 'conversation', slot: 'column', column: 1, props: { agents: 'agents' }, caps: [], source: 'builtin' }
/** The template's thin left rail (a column strip): it gives the page the rail views. */
const STRIP_RAIL = { id: 'nav', kind: 'nav-rail', slot: 'column', column: 0, props: { orientation: 'column' }, caps: ['app:navigate'], source: 'builtin' }

const SWITCHER = (slot: string, extra: Record<string, unknown> = {}) => ({
  id: 'garden-switcher',
  kind: 'garden-switcher',
  slot,
  props: {},
  caps: ['gardens:manage'],
  ...extra,
})
const item = (kind: string, slot: string, extra: Record<string, unknown> = {}) => ({ id: kind, kind, slot, props: {}, caps: [], ...extra })
const MORE = { id: 'more', kind: 'menu', slot: 'top', align: 'end', props: { icon: 'dots', label: 'More' }, caps: [] }

const TEMPLATE_ITEMS = [
  SWITCHER('top', { align: 'start' }),
  item('usage', 'top', { align: 'end' }),
  item('theme-picker', 'top', { align: 'end' }),
  item('zen-toggle', 'top', { align: 'end' }),
]
const TEMPLATE_CONTROLS = ['garden-switcher', 'drag-region', 'zen-toggle', 'add-agent']

interface Example {
  widgets: unknown[]
  from: 'template' | 'garden'
  items: unknown[]
  bands: { top: Groups | null; bottom: Groups | null }
  edges?: unknown[]
  menus?: Record<string, string[]>
}

/** The FC34 guide examples, as `GET /cli/zen/get` answers them. */
const EXAMPLES: Record<string, Example> = {
  'rail-top': {
    widgets: [AGENTS, CONVERSATION, { id: 'nav-rail', kind: 'nav-rail', slot: 'top', props: { orientation: 'row' }, caps: ['app:navigate'], source: 'builtin' }],
    from: 'template',
    items: TEMPLATE_ITEMS,
    bands: { top: g(['garden-switcher', 'nav-rail'], [], ['usage', 'theme-picker', 'zen-toggle']), bottom: null },
  },
  'quiet-top': {
    widgets: [AGENTS, CONVERSATION, STRIP_RAIL],
    from: 'garden',
    items: [SWITCHER('top', { align: 'start' }), item('theme-picker', 'top', { align: 'end' }), item('zen-toggle', 'top', { align: 'end' })],
    bands: { top: g(['garden-switcher'], [], ['theme-picker', 'zen-toggle']), bottom: null },
  },
  'bottom-bar': {
    widgets: [AGENTS, CONVERSATION, STRIP_RAIL],
    from: 'garden',
    items: [SWITCHER('bottom', { align: 'start' }), item('theme-picker', 'bottom', { align: 'end' }), item('zen-toggle', 'bottom', { align: 'end' })],
    bands: { top: null, bottom: g(['garden-switcher'], [], ['theme-picker', 'zen-toggle']) },
  },
  'menu-both': {
    widgets: [AGENTS, CONVERSATION, STRIP_RAIL],
    from: 'garden',
    items: [MORE, SWITCHER('menu', { menu: 'more' }), item('theme-picker', 'menu', { menu: 'more' }), item('zen-toggle', 'menu', { menu: 'more' })],
    bands: { top: g([], [], ['more']), bottom: null },
    menus: { more: ['garden-switcher', 'theme-picker', 'zen-toggle'] },
  },
  'texting-chrome': {
    widgets: [AGENTS, CONVERSATION, STRIP_RAIL],
    from: 'garden',
    items: TEMPLATE_ITEMS,
    bands: { top: g(['garden-switcher'], [], ['usage', 'theme-picker', 'zen-toggle']), bottom: null },
  },
  'column-corner': {
    widgets: [AGENTS, CONVERSATION, STRIP_RAIL],
    from: 'garden',
    items: [SWITCHER('top', { align: 'start' }), item('zen-toggle', 'column', { column: 1, edge: 'bottom', align: 'end' })],
    bands: { top: g(['garden-switcher'], [], []), bottom: null },
    edges: [{ column: 1, edge: 'bottom', start: [], center: [], end: ['zen-toggle'] }],
  },
}

function answer(ex: Example, opts: { chrome?: boolean } = {}): Record<string, unknown> {
  const page: Record<string, unknown> = {
    template: 'k2.texting@1',
    layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
    widgets: ex.widgets,
    controls: TEMPLATE_CONTROLS,
  }
  if (opts.chrome !== false) {
    page.chrome = { from: ex.from, items: ex.items }
    page.bands = ex.bands
    page.edges = ex.edges ?? []
    page.menus = ex.menus ?? {}
  }
  return {
    ok: true,
    schema: 1,
    version: 'v1',
    garden: { id: 'g-1', name: 'Garden 1', index: 1 },
    page,
    theme: { name: 'basic' },
    themes: [
      { name: 'basic', label: 'Basic', builtin: true, user: false },
      { name: 'paper', label: 'Paper', builtin: true, user: false },
    ],
    chrome: {},
    motion: {},
    errors: [],
    warnings: [],
    lastGoodAt: null,
  }
}

// ── Fake layout ─────────────────────────────────────────────────────────

const styleOverride = new Map<Element, Partial<{ display: string; visibility: string; opacity: number }>>()
const rectOverride = new Map<Element, ZenRect>()
/** What `elementFromPoint` finds at an element's centre instead of it. */
const coveredBy = new Map<Element, () => Element | null>()
let lastRected: Element | null = null
const fakeGeometry: ZenGeometry = {
  rect(el) {
    lastRected = el
    return rectOverride.get(el) ?? { left: 120, top: 60, width: 80, height: 30 }
  },
  style(el) {
    return { display: 'block', visibility: 'visible', opacity: 1, ...styleOverride.get(el) }
  },
  viewport() {
    return { width: 1200, height: 800 }
  },
  elementFromPoint() {
    if (!lastRected) return null
    const cover = coveredBy.get(lastRected)
    return cover ? cover() : lastRected
  },
}

beforeEach(() => {
  h.calls.length = 0
  h.page = null
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  __setZenGeometryForTests(fakeGeometry)
  styleOverride.clear()
  rectOverride.clear()
  coveredBy.clear()
  __resetZenApiForTests()
  __resetZenGardensForTests()
  localStorage.clear()
  useZenOverlayStore.setState({ picker: false, sheet: false })
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('agents')
  useZenWindowStore.setState({ on: true, garden: 'g-1', view: 'home' })
})

afterEach(() => {
  cleanup()
  __setZenGeometryForTests(null)
})

async function show(example: string, opts: { chrome?: boolean } = {}): Promise<void> {
  h.page = answer(EXAMPLES[example], opts)
  render(<ZenHost />)
  await waitFor(() => {
    if (!document.querySelector('[data-zen-page="k2.texting@1"]')) throw new Error('Zen page not drawn')
  })
}

function el(selector: string): HTMLElement {
  const found = document.querySelector(selector)
  if (!(found instanceof HTMLElement)) throw new Error(`no ${selector}\n${document.body.innerHTML.slice(0, 1500)}`)
  return found
}

/** A row, left to right: each chrome item by kind, a run of content
 *  widgets as `band:<slot>`, the empty drag space as `drag`. */
function row(selector: string): string[] {
  const inner = el(`${selector} [data-zen-row-inner]`)
  const out: string[] = []
  for (const c of Array.from(inner.children)) {
    if (c.hasAttribute('data-zen-row-spacer')) {
      out.push('drag')
      continue
    }
    for (const x of Array.from(c.children)) {
      out.push(x.getAttribute('data-zen-chrome-kind') ?? (x.hasAttribute('data-zen-band') ? `band:${x.getAttribute('data-zen-band')}` : x.tagName))
    }
  }
  return out
}

function twoChecks(): void {
  act(() => runZenControlChecksNow())
  act(() => runZenControlChecksNow())
}

function expectNoSafeMode(what: string): void {
  expect(useZenViewStore.getState().safe, what).toBeNull()
  expect(document.querySelector('[data-zen-safe-banner]'), what).toBeNull()
}

function safeReason(): string {
  const r = document.querySelector('[data-zen-safe-reason]')
  if (!r) throw new Error(`not in safe mode\n${document.body.innerHTML.slice(0, 1500)}`)
  return r.textContent ?? ''
}

function expectEveryButtonPointer(): void {
  const bare = Array.from(document.querySelectorAll('[data-zen-root] button')).filter((b) => !b.classList.contains('cursor-pointer'))
  expect(bare.map((b) => b.outerHTML.slice(0, 120))).toEqual([])
}

describe('each guide example draws its layout from the page answer (FC34, S2)', () => {
  it('rail-top: the template band, the rail right of the switcher (it does NOT remove the switcher)', async () => {
    await show('rail-top')
    expect(el('[data-zen-page]').getAttribute('data-zen-chrome-from')).toBe('template')
    expect(row('[data-zen-band-row="top"]')).toEqual(['garden-switcher', 'band:top', 'drag', 'usage', 'theme-picker', 'zen-toggle'])
    expect(document.querySelector('[data-zen-band-row="bottom"]')).toBeNull()
    expect(document.querySelector('[data-zen-title-strip]')).toBeNull()
    twoChecks()
    expectNoSafeMode('rail-top')
  })

  it('quiet-top: the template band without usage', async () => {
    await show('quiet-top')
    expect(el('[data-zen-page]').getAttribute('data-zen-chrome-from')).toBe('garden')
    expect(row('[data-zen-band-row="top"]')).toEqual(['garden-switcher', 'drag', 'theme-picker', 'zen-toggle'])
    expect(document.querySelector('[data-zen-usage]')).toBeNull()
    twoChecks()
    expectNoSafeMode('quiet-top')
  })

  it('bottom-bar: no top band (the title strip stays), the switcher bottom left, theme and toggle bottom right, no usage', async () => {
    await show('bottom-bar')
    expect(document.querySelector('[data-zen-band-row="top"]')).toBeNull()
    const strip = el('[data-zen-title-strip]')
    expect(strip.getAttribute('data-zen-bound')).toBe('drag-region')
    expect(row('[data-zen-band-row="bottom"]')).toEqual(['garden-switcher', 'drag', 'theme-picker', 'zen-toggle'])
    const bottom = el('[data-zen-band-row="bottom"]')
    expect(bottom.getAttribute('data-zen-bound')).toBe('drag-region')
    // The last `end` item is in the corner.
    expect(bottom.querySelector('[data-zen-row-group="end"]')?.lastElementChild?.getAttribute('data-zen-chrome-kind')).toBe('zen-toggle')
    // Below the columns, the page's last row.
    expect(el('[data-zen-page]').lastElementChild).toBe(bottom)
    expect(document.querySelector('[data-zen-usage]')).toBeNull()
    expect(el('[data-zen-garden-pill]').getAttribute('data-zen-bound')).toBe('garden-switcher')
    expect(el('[data-zen-switch]').getAttribute('data-zen-bound')).toBe('zen-toggle')
    expectEveryButtonPointer()
    twoChecks()
    expectNoSafeMode('bottom-bar')
  })

  it('menu-both: one ⋯ button top right; closed, neither required control is on the page, and the check passes (FC-T7)', async () => {
    await show('menu-both')
    expect(row('[data-zen-band-row="top"]')).toEqual(['drag', 'menu'])
    const button = el('[data-zen-menu-button="more"]')
    expect([button.getAttribute('aria-label'), button.getAttribute('title'), button.getAttribute('aria-haspopup'), button.getAttribute('aria-expanded')]).toEqual([
      'More',
      'More',
      'menu',
      'false',
    ])
    expect(button.getAttribute('data-zen-bound')).toBe('zen-menu')
    expect(button.hasAttribute('data-zen-glass')).toBe(true)
    expect(button.classList.contains('cursor-pointer')).toBe(true)
    expect(button.querySelector('[data-zen-menu-icon="dots"]')).not.toBeNull()
    // Closed: no toggle, no switcher on the page at all.
    expect(document.querySelector('[data-zen-switch], [data-zen-garden-pill], [data-zen-bound="zen-toggle"], [data-zen-bound="garden-option"]')).toBeNull()
    twoChecks()
    expectNoSafeMode('menu-both, closed')
  })

  it('texting-chrome: the template’s chrome written out looks exactly like the template', async () => {
    await show('texting-chrome')
    expect(el('[data-zen-page]').getAttribute('data-zen-chrome-from')).toBe('garden')
    expect(row('[data-zen-band-row="top"]')).toEqual(['garden-switcher', 'drag', 'usage', 'theme-picker', 'zen-toggle'])
    twoChecks()
    expectNoSafeMode('texting-chrome')
  })

  it('column-corner: the toggle at the bottom-right edge of the conversation column, outside its box', async () => {
    await show('column-corner')
    expect(row('[data-zen-band-row="top"]')).toEqual(['garden-switcher', 'drag'])
    const slot1 = el('[data-zen-column-slot="1"]')
    expect(slot1.classList.contains('flex-col')).toBe(true)
    const edge = el('[data-zen-edge-row="1:bottom"]')
    expect(slot1.lastElementChild).toBe(edge)
    expect(edge.querySelector('[data-zen-row-group="end"] [data-zen-switch]')).not.toBeNull()
    // Column edges are not drag areas.
    expect(edge.hasAttribute('data-zen-bound')).toBe(false)
    expect(edge.querySelector('[data-zen-drag]')).toBeNull()
    expect(el('[data-zen-column="1"]').querySelector('[data-zen-switch]')).toBeNull()
    twoChecks()
    expectNoSafeMode('column-corner')
  })
})

describe('older daemons and the title strip (FC32, FC10)', () => {
  it('an answer without page.chrome draws the template’s top band', async () => {
    await show('bottom-bar', { chrome: false })
    expect(el('[data-zen-page]').getAttribute('data-zen-chrome-from')).toBe('template')
    expect(row('[data-zen-band-row="top"]')).toEqual(['garden-switcher', 'drag', 'usage', 'theme-picker', 'zen-toggle'])
    expect(document.querySelector('[data-zen-band-row="bottom"]')).toBeNull()
    twoChecks()
    expectNoSafeMode('older daemon')
  })

  it('no top band still leaves the title strip: empty, draggable, as tall as the window buttons', async () => {
    await show('bottom-bar')
    const strip = el('[data-zen-title-strip]')
    expect(el('[data-zen-page]').querySelector(':scope > [data-zen-title-strip]')).toBe(strip)
    expect(strip.style.height).toBe('var(--zen-stoplight-safe-top, 28px)')
    expect(strip.children.length).toBe(0)
    expect(strip.getAttribute('data-zen-bound')).toBe('drag-region')
    // The columns start below it: it comes before the layout host.
    const layout = el('[data-zen-layout]')
    expect(strip.compareDocumentPosition(layout) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })
})

describe('the required-controls check reads where the page put them (FC25, S3)', () => {
  it('a hidden required control fails: a direct toggle, and a menu’s button (named)', async () => {
    await show('bottom-bar')
    styleOverride.set(el('[data-zen-switch]'), { opacity: 0 })
    twoChecks()
    expect(safeReason()).toBe('The Zen toggle isn’t visible.')
  })

  it('menu-both: the menu button at opacity 0.2 → “The Zen toggle’s menu (More) isn’t visible.”', async () => {
    await show('menu-both')
    styleOverride.set(el('[data-zen-menu-button="more"]'), { opacity: 0.2 })
    twoChecks()
    expect(safeReason()).toBe('The Zen toggle’s menu (More) isn’t visible.')
  })

  it('menu-both: a menu button off the keyboard → no-keyboard', async () => {
    await show('menu-both')
    el('[data-zen-menu-button="more"]').tabIndex = -1
    twoChecks()
    expect(safeReason()).toBe('The Zen toggle’s menu (More) can’t be reached from the keyboard.')
  })

  it('FC-T9: an open bottom-band menu over a column-edge toggle is not covering it', async () => {
    h.page = answer({
      widgets: [AGENTS, CONVERSATION, STRIP_RAIL],
      from: 'garden',
      items: [
        { ...MORE, slot: 'bottom', align: 'start' },
        SWITCHER('menu', { menu: 'more' }),
        item('zen-toggle', 'column', { column: 1, edge: 'bottom', align: 'end' }),
      ],
      bands: { top: null, bottom: g(['more'], [], []) },
      edges: [{ column: 1, edge: 'bottom', start: [], center: [], end: ['zen-toggle'] }],
      menus: { more: ['garden-switcher'] },
    })
    render(<ZenHost />)
    await waitFor(() => el('[data-zen-menu-button="more"]'))
    await act(async () => void fireEvent.click(el('[data-zen-menu-button="more"]')))
    const list = el('[data-zen-menu-list="more"]')
    // The open list sits over the toggle.
    coveredBy.set(el('[data-zen-switch]'), () => list)
    twoChecks()
    expectNoSafeMode('open menu over the toggle')
    // A page element over it does cover it.
    coveredBy.set(el('[data-zen-switch]'), () => document.body)
    twoChecks()
    expect(safeReason()).toBe('The Zen toggle isn’t visible.')
  })

  it('opening and closing a menu quickly, then the checks: never a false alarm (FC26)', async () => {
    await show('menu-both')
    const button = el('[data-zen-menu-button="more"]')
    await act(async () => void fireEvent.click(button))
    expect(el('[data-zen-menu-list="more"]').getAttribute('role')).toBe('menu')
    // Open: the rows are K2's own overlay, the button is what is checked.
    twoChecks()
    expectNoSafeMode('menu open')
    await act(async () => void fireEvent.click(button))
    expect(document.querySelector('[data-zen-menu-list]')).toBeNull()
    await act(async () => {
      await new Promise((r) => setTimeout(r, ZEN_WIRING_DEADLINE_MS + 100))
    })
    twoChecks()
    expectNoSafeMode('menu opened and closed')
  }, 10_000)

  it('a menu that draws its rows without binding the toggle → not wired after its button, naming the menu', async () => {
    // The Exit row drawn by a broken chrome entry would never bind; here
    // the page's menu holds the toggle but the menu kind is swapped for one
    // that lists only Gardens.
    const off = registerZenChrome('menu', function GardensOnly({ item: menuItem, bridge }: ZenChromeProps) {
      const [open, setOpen] = useState(false)
      const bind = useZenBind(bridge, 'zen-menu', menuItem.id)
      return (
        <div>
          <button ref={bind} type="button" className="cursor-pointer" data-test-menu="" onClick={() => setOpen((v) => !v)}>
            More
          </button>
          {open && bridge.gardens.list().map((x) => <GardenRow key={x.id} id={x.id} bridge={bridge} />)}
        </div>
      )
    })
    try {
      await show('menu-both')
      twoChecks()
      expectNoSafeMode('shown, not yet activated')
      await act(async () => void fireEvent.click(el('[data-test-menu]')))
      await act(async () => {
        await new Promise((r) => setTimeout(r, ZEN_WIRING_DEADLINE_MS + 100))
      })
      twoChecks()
      expect(safeReason()).toBe('The Zen toggle’s menu (More) isn’t wired.')
    } finally {
      off()
    }
  }, 10_000)
})

function GardenRow({ id, bridge }: { id: string; bridge: ZenChromeProps['bridge'] }): React.JSX.Element {
  const ref = useZenBind(bridge, 'garden-option', id)
  return (
    <button ref={ref} type="button" className="cursor-pointer">
      {id}
    </button>
  )
}

describe('keyboard (FC19, FC-T12)', () => {
  it('Tab to the menu button, Enter opens it on the first Garden, ↓ walks to Exit Zen Mode, Enter exits', async () => {
    await show('menu-both')
    const button = el('[data-zen-menu-button="more"]')
    expect(button.tabIndex).toBe(0)
    act(() => button.focus())
    expect(document.activeElement).toBe(button)
    // Enter on a button clicks it natively; the menu never cancels the key.
    const enter = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
    await act(async () => void button.dispatchEvent(enter))
    expect(enter.defaultPrevented).toBe(false)
    await act(async () => void fireEvent.click(button))
    expect(button.getAttribute('aria-expanded')).toBe('true')
    const list = el('[data-zen-menu-list="more"]')
    expect(list.getAttribute('role')).toBe('menu')
    expect(document.activeElement?.getAttribute('data-zen-garden-option')).toBe('g-1')
    const focused = (): string =>
      document.activeElement?.getAttribute('data-zen-garden-option') ??
      (document.activeElement?.hasAttribute('data-zen-new-garden')
        ? 'new'
        : (document.activeElement?.getAttribute('data-zen-menu-panel-row') ??
          (document.activeElement?.hasAttribute('data-zen-menu-exit') ? 'exit' : String(document.activeElement?.tagName))))
    const walked: string[] = [focused()]
    for (let i = 0; i < 4; i++) {
      await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowDown' }))
      walked.push(focused())
    }
    expect(walked).toEqual(['g-1', 'g-2', 'new', 'theme', 'exit'])
    // ↓ wraps; Home / End jump; ↑ wraps back.
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowDown' }))
    expect(focused()).toBe('g-1')
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowUp' }))
    expect(focused()).toBe('exit')
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Home' }))
    expect(focused()).toBe('g-1')
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'End' }))
    expect(focused()).toBe('exit')
    expect(el('[data-zen-menu-exit]').textContent).toBe('Exit Zen Mode⌃⌘Z')
    expect(el('[data-zen-menu-exit]').getAttribute('role')).toBe('menuitem')
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Enter' }))
    expect(useZenWindowStore.getState().on).toBe(false)
    expect(document.querySelector('[data-zen-root]')).toBeNull()
  })

  it('Esc closes and puts focus back on the button; the Theme panel goes back with ← or Esc; Tab closes', async () => {
    await show('menu-both')
    const button = el('[data-zen-menu-button="more"]')
    act(() => button.focus())
    await act(async () => void fireEvent.keyDown(button, { key: 'ArrowDown' }))
    expect(button.getAttribute('aria-expanded')).toBe('true')
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Escape' }))
    expect(document.querySelector('[data-zen-menu-list]')).toBeNull()
    expect(document.activeElement).toBe(button)

    // Theme: <name> › swaps to the theme list, Back first.
    await act(async () => void fireEvent.click(button))
    const themeRow = el('[data-zen-menu-panel-row="theme"]')
    expect(themeRow.textContent).toBe('Theme: Basic›')
    act(() => themeRow.focus())
    await act(async () => void fireEvent.keyDown(themeRow, { key: 'Enter' }))
    expect(el('[data-zen-menu-list="more"]').getAttribute('data-zen-menu-panel')).toBe('theme')
    expect(document.activeElement?.hasAttribute('data-zen-menu-back')).toBe(true)
    expect(Array.from(document.querySelectorAll('[data-zen-menu-theme]')).map((t) => [t.getAttribute('data-zen-menu-theme'), t.getAttribute('aria-checked')])).toEqual([
      ['basic', 'true'],
      ['paper', 'false'],
    ])
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowLeft' }))
    expect(el('[data-zen-menu-list="more"]').getAttribute('data-zen-menu-panel')).toBe('top')
    expect(document.activeElement?.getAttribute('data-zen-menu-panel-row')).toBe('theme')
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Enter' }))
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Escape' }))
    expect(el('[data-zen-menu-list="more"]').getAttribute('data-zen-menu-panel')).toBe('top')
    // A pick switches the theme on this computer's daemon and closes.
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Enter' }))
    await act(async () => void fireEvent.click(el('[data-zen-menu-theme="paper"]')))
    await waitFor(() => expect(h.calls.filter((c) => c.method === 'POST').map((c) => [c.route, c.data])).toEqual([['zen/theme/set', { name: 'paper' }]]))
    expect(document.querySelector('[data-zen-menu-list]')).toBeNull()

    // Tab closes it.
    await act(async () => void fireEvent.click(button))
    await act(async () => void fireEvent.keyDown(document.activeElement as Element, { key: 'Tab' }))
    expect(document.querySelector('[data-zen-menu-list]')).toBeNull()
    expect(document.activeElement).toBe(button)
    expectEveryButtonPointer()
  })

  it('a Garden picked from the menu switches this window', async () => {
    await show('menu-both')
    await act(async () => void fireEvent.click(el('[data-zen-menu-button="more"]')))
    const options = Array.from(document.querySelectorAll('[data-zen-menu-list] [data-zen-garden-option]'))
    expect(options.map((o) => [o.getAttribute('data-zen-garden-option'), o.getAttribute('role'), o.getAttribute('data-zen-bound')])).toEqual([
      ['g-1', 'menuitemradio', 'garden-option'],
      ['g-2', 'menuitemradio', 'garden-option'],
    ])
    expect(options[1].textContent).toContain('⌥⌘2')
    expect(document.querySelector('[data-zen-menu-list] [data-zen-new-garden]')).not.toBeNull()
    await act(async () => void fireEvent.click(options[1]))
    expect(useZenWindowStore.getState().garden).toBe('g-2')
  })
})

describe('overflow and the column clamp (FC27, FC-T10)', () => {
  function setWidth(node: Element | null, prop: 'clientWidth' | 'offsetWidth', value: number): void {
    if (!node) throw new Error(`no element for ${prop}`)
    Object.defineProperty(node, prop, { value, configurable: true })
  }

  it('a crowded band hides usage, then theme, then truncates the switcher; the toggle and switcher stay', async () => {
    await show('texting-chrome')
    const band = el('[data-zen-band-row="top"]')
    const inner = el('[data-zen-band-row="top"] [data-zen-row-inner]')
    const widths: Record<string, number> = { 'garden-switcher': 120, usage: 100, 'theme-picker': 90, 'zen-toggle': 76 }
    for (const [kind, w] of Object.entries(widths)) setWidth(band.querySelector(`[data-zen-chrome-kind="${kind}"]`)?.firstElementChild ?? null, 'offsetWidth', w)
    const hidden = (): string[] =>
      Array.from(band.querySelectorAll('[data-zen-overflow-hidden]')).map((x) => x.getAttribute('data-zen-chrome-kind') ?? '')
    const resize = async (w: number): Promise<void> => {
      setWidth(inner, 'clientWidth', w)
      await act(async () => void window.dispatchEvent(new Event('resize')))
    }
    // fixed: 2 group gaps (24) + switcher 128 + toggle 84 + 120 drag = 356;
    // usage 108, theme 98 → 562 in all.
    await resize(600)
    expect(hidden()).toEqual([])
    await resize(500)
    expect(hidden()).toEqual(['usage'])
    expect((band.querySelector('[data-zen-chrome-kind="usage"]') as HTMLElement).style.display).toBe('none')
    await resize(400)
    expect(hidden()).toEqual(['usage', 'theme-picker'])
    expect(el('[data-zen-garden-pill]').hasAttribute('data-zen-compact')).toBe(false)
    await resize(300)
    expect(hidden()).toEqual(['usage', 'theme-picker'])
    expect(el('[data-zen-garden-pill]').hasAttribute('data-zen-compact')).toBe(true)
    expect((el('[data-zen-garden-pill]').firstElementChild as HTMLElement).style.maxWidth).toBe('6rem')
    // The required controls never hide.
    for (const kind of ['garden-switcher', 'zen-toggle']) {
      expect((band.querySelector(`[data-zen-chrome-kind="${kind}"]`) as HTMLElement).style.display, kind).toBe('contents')
    }
    twoChecks()
    expectNoSafeMode('narrow band')
    // Wide again: everything comes back.
    await resize(700)
    expect(hidden()).toEqual([])
    expect(el('[data-zen-garden-pill]').hasAttribute('data-zen-compact')).toBe(false)
  })

  it('every column’s min width is clamped to its share of the row', async () => {
    await show('column-corner')
    expect(el('[data-zen-column-slot="0"]').style.minWidth).toBe('min(240px, calc((100% - 1 * var(--zen-gap)) * 0.4))')
    expect(el('[data-zen-column-slot="1"]').style.minWidth).toBe('min(360px, calc((100% - 1 * var(--zen-gap)) * 0.6))')
  })
})

describe('dropdowns open toward the page (FC18, FC20, FC-T11)', () => {
  function atBottom(node: HTMLElement): void {
    node.getBoundingClientRect = () => ({ top: 730, bottom: 760, left: 20, right: 140, width: 120, height: 30, x: 20, y: 730, toJSON: () => ({}) }) as DOMRect
  }

  it('in the bottom band the Garden switcher and the theme list open up; the usage menu flips through Zen CSS', async () => {
    await show('bottom-bar')
    atBottom(el('[data-zen-garden-switcher]'))
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    const menu = el('[data-zen-garden-menu]')
    expect(menu.getAttribute('data-zen-menu-placement')).toBe('up')
    expect(menu.style.bottom).not.toBe('')
    expect(menu.style.top).toBe('')
    // Portalled into the Zen root, a registered K2 overlay.
    expect(menu.parentElement?.hasAttribute('data-zen-root')).toBe(true)
    await act(async () => void fireEvent.mouseDown(document.body))
    expect(document.querySelector('[data-zen-garden-menu]')).toBeNull()

    atBottom(el('[data-zen-theme-picker]'))
    await act(async () => void fireEvent.click(el('[data-zen-theme-button]')))
    expect(el('[data-zen-theme-list]').getAttribute('data-zen-menu-placement')).toBe('up')
    expect(document.querySelectorAll('[data-zen-theme-option]').length).toBe(2)

    // The usage menu is the app's component: Zen flips it with its CSS.
    expect(ZEN_USAGE_CSS).toContain('[data-zen-usage][data-zen-usage-opens="up"] [data-testid="subscription-usage-menu"]')
    expect(ZEN_USAGE_CSS).toMatch(/data-zen-usage-opens="up"\] \[data-testid="subscription-usage-menu"\] \{\s*top: auto;\s*bottom: 100%;/)
  })

  it('a usage chip in the bottom band is marked to open up; in the top band, down', async () => {
    h.page = answer({
      ...EXAMPLES['bottom-bar'],
      items: [...EXAMPLES['bottom-bar'].items, item('usage', 'bottom', { align: 'start' })],
      bands: { top: null, bottom: g(['garden-switcher', 'usage'], [], ['theme-picker', 'zen-toggle']) },
    })
    render(<ZenHost />)
    await waitFor(() => el('[data-zen-usage]'))
    expect([el('[data-zen-usage]').getAttribute('data-zen-usage-opens'), el('[data-zen-usage]').getAttribute('data-zen-usage-align')]).toEqual(['up', 'start'])
    cleanup()
    await show('texting-chrome')
    expect([el('[data-zen-usage]').getAttribute('data-zen-usage-opens'), el('[data-zen-usage]').getAttribute('data-zen-usage-align')]).toEqual(['down', 'end'])
  })

  it('the top band’s dropdowns still open down', async () => {
    await show('texting-chrome')
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    expect(el('[data-zen-garden-menu]').getAttribute('data-zen-menu-placement')).toBe('down')
  })
})

describe('safe mode and rail views (FC22, FC26, FC48)', () => {
  it('FC-T13: Shift on entry with a bottom-bar Garden draws the template’s top band, and no bottom band', async () => {
    h.page = answer(EXAMPLES['bottom-bar'])
    useZenWindowStore.setState({ on: false })
    render(<ZenHost />)
    act(() => enterZen({ safe: true }))
    await waitFor(() => el('[data-zen-safe-banner]'))
    expect(row('[data-zen-band-row="top"]')).toEqual(['garden-switcher', 'drag', 'zen-toggle'])
    expect(document.querySelector('[data-zen-band-row="bottom"]')).toBeNull()
    expect(document.querySelector('[data-zen-usage], [data-zen-theme-picker]')).toBeNull()
    // Safe mode never read the Garden's page.
    expect(h.calls.filter((c) => c.route === 'zen/get')).toEqual([])
  })

  it('FC-T14: the Tickets view keeps the bottom band, and switching views never trips the check', async () => {
    await show('bottom-bar')
    twoChecks()
    for (const view of ['tickets', 'projects', 'agents', 'home'] as const) {
      act(() => useZenWindowStore.getState().setView(view))
      await waitFor(() => expect(el('[data-zen-page]').getAttribute('data-zen-view')).toBe(view))
      expect(row('[data-zen-band-row="bottom"]'), view).toEqual(['garden-switcher', 'drag', 'theme-picker', 'zen-toggle'])
      twoChecks()
      expectNoSafeMode(view)
    }
  })

  it('FC-T14 / FC48: on a column-corner page the toggle moves to column 0’s bottom edge in Tickets, and stays checked', async () => {
    await show('column-corner')
    act(() => useZenWindowStore.getState().setView('tickets'))
    await waitFor(() => expect(el('[data-zen-page]').getAttribute('data-zen-view')).toBe('tickets'))
    expect(document.querySelectorAll('[data-zen-column-slot]').length).toBe(1)
    const edge = el('[data-zen-edge-row="0:bottom"]')
    expect(edge.querySelector('[data-zen-row-group="end"] [data-zen-switch]')).not.toBeNull()
    expect(el('[data-zen-switch]').getAttribute('data-zen-bound')).toBe('zen-toggle')
    twoChecks()
    expectNoSafeMode('column-corner tickets')
    act(() => useZenWindowStore.getState().setView('home'))
    await waitFor(() => el('[data-zen-edge-row="1:bottom"] [data-zen-switch]'))
    twoChecks()
    expectNoSafeMode('column-corner home')
  })
})
