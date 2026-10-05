// @vitest-environment jsdom
//
// prd-zen-mode-v1 S4 and prd-zen-gardens-v1 S3/S4 — window-level Zen and
// Gardens through the real ZenHost, Zen root, page, template controls,
// bridge, control registry and safe mode. Only the edges are faked: the
// daemon (`daemon-cli`), the app socket (`session-events`), Tauri, and
// layout (jsdom has none, so the control check reads an injected geometry).
//
// Asserted (fail loudly):
//   - the top-bar toggle turns THIS window's Zen on over whatever page it is
//     on, and every Zen request goes to this computer's daemon (G1–G5,
//     TG3.2); Zen on with no folder sets it up once (G22);
//   - exit shows the same page; Settings only hides Zen; a page change
//     turns Zen off (G34); a new window starts off, a relaunch keeps its
//     switch (G1); keyboard focus moves into Zen (G50);
//   - the Garden switcher lists every Garden, switches this window's Garden,
//     and "+ New Garden" asks how it starts (Start with the default = the
//     texting page, selected; Start empty and ask my agent = blank), creates
//     on the local daemon, switches to the new Garden and shows the clash
//     copy (G25, TG4.2; Rosson 2026-10-04); a Garden deleted elsewhere moves
//     the window to the first one (TG4.5);
//   - a missing / invisible / unwired / undeclared Garden switcher, or one
//     whose options miss a Garden, is safe mode after two failed checks, not
//     one (G24, TG4.1); Shift, a crash, an unreachable or older daemon are
//     safe mode with their cause;
//   - the escape hatch (menu event, macOS targeted menu, Linux Ctrl+Alt+Z in
//     the capture phase, AltGr ignored) always exits, safe mode included,
//     and never changes the page.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Garden = { id: string; name: string; template: string; seedHome?: string }

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  getImpl: null as null | ((route: string, params: unknown) => unknown),
  postImpl: null as null | ((route: string, body: unknown) => unknown),
  gardens: [] as Garden[],
  setUp: true,
  zenHandlers: [] as Array<{ hostKey: string; fn: () => void }>,
  sockets: [] as string[],
  closedSockets: [] as string[],
  invokes: [] as Array<{ cmd: string; args: unknown }>,
  tauriListens: [] as Array<{ event: string; handler: () => void; options: unknown }>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: unknown) => {
    h.invokes.push({ cmd, args })
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async (event: string, handler: () => void, options?: unknown) => {
    h.tauriListens.push({ event, handler, options })
    return () => {
      const i = h.tauriListens.findIndex((l) => l.handler === handler)
      if (i >= 0) h.tauriListens.splice(i, 1)
    }
  }),
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
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (!h.getImpl) throw new Error(`unexpected GET ${route}`)
    return h.getImpl(route, params)
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    if (!h.postImpl) throw new Error(`unexpected POST ${route}`)
    return h.postImpl(route, body)
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...mod,
    onZenChanged: (scope: { hostKey: string }, fn: () => void) => {
      const entry = { hostKey: scope.hostKey, fn }
      h.zenHandlers.push(entry)
      return () => {
        const i = h.zenHandlers.indexOf(entry)
        if (i >= 0) h.zenHandlers.splice(i, 1)
      }
    },
    subscribeToActiveState: (scope: { hostKey: string }) => {
      h.sockets.push(scope.hostKey)
      return () => void h.closedSockets.push(scope.hostKey)
    },
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __resetZenGardensForTests, useZenGardensStore } from '@/lib/zen/zen-gardens'
import { __reloadZenWindowForTests, useZenWindowStore, zenWindowKey } from '@/lib/zen/zen-window'
import { __setZenGeometryForTests, runZenControlChecksNow } from '@/lib/zen/zen-monitor'
import { ZEN_WIRING_DEADLINE_MS, type ZenGeometry, type ZenRect } from '@/lib/zen/zen-controls'
import { useZenViewStore, zenShownNow } from '@/lib/zen/zen-view'
import { ZenHost } from './ZenHost'
import ZenTopBarToggle from '@/components/TopBar/ZenTopBarToggle'
import { registerZenTemplateControls, registerZenWidget, type ZenTemplateControlsProps } from './zen-registry'
import { useZenBind } from './ZenTemplateControls'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const DEFAULT: Garden = { id: 'g-default', name: 'Garden 1', template: 'k2.texting@1' }
const MORNINGS: Garden = { id: 'g-mornings', name: 'Mornings', template: 'k2.blank@1' }
/** The second Garden setup makes (Rosson 2026-10-04): empty, to build. */
const GARDEN_2: Garden = { id: 'g-garden2', name: 'Garden 2', template: 'k2.blank@1' }

/** `GET /cli/zen/gardens` as the daemon answers it (G13). */
function gardensAnswer(): unknown {
  return {
    ok: true,
    setUp: h.setUp,
    gardens: h.gardens.map((g, i) => ({ ...g, index: i + 1, hasFile: true, createdAt: '2026-10-04T18:00:00Z' })),
  }
}

/** A resolved Garden page as S1 answers it (docs/zen-contract.md). */
function gardenPage(id: string, version = 'v1'): unknown {
  const g = h.gardens.find((x) => x.id === id)
  if (!g) throw new Error('unknown_garden')
  const blank = g.template === 'k2.blank@1'
  return {
    ok: true,
    schema: 1,
    version,
    garden: { id: g.id, name: g.name, index: h.gardens.indexOf(g) + 1 },
    page: blank
      ? {
          template: 'k2.blank@1',
          layout: { kind: 'columns', split: [100], minWidths: [0] },
          widgets: [{ id: 'garden-empty', kind: 'garden-empty', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' }],
          controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
        }
      : {
          template: 'k2.texting@1',
          layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
          widgets: [
            { id: 'agents', kind: 'agents', column: 0, props: { 'home-picker': true }, caps: ['agents:read'], source: 'builtin' },
            { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: ['thread:read'], source: 'builtin' },
          ],
          controls: ['garden-switcher', 'drag-region', 'zen-toggle', 'add-agent'],
        },
    theme: {},
    chrome: {},
    motion: {},
    errors: [],
    warnings: [],
    lastGoodAt: null,
  }
}

function defaultGet(route: string, params: unknown): unknown {
  if (route === 'zen/gardens') return gardensAnswer()
  if (route === 'zen/get') return gardenPage(String((params as { garden?: string }).garden))
  throw new Error(`unexpected GET ${route}`)
}

function defaultPost(route: string, body: unknown): unknown {
  if (route === 'zen/setup') {
    h.setUp = true
    if (h.gardens.length === 0) h.gardens.push({ ...DEFAULT }, { ...GARDEN_2 })
    return { ok: true, createdFolder: true, migrated: null, gardens: [] }
  }
  if (route === 'zen/garden/new') {
    const name = String((body as { name: string }).name)
    if (h.gardens.some((g) => g.name.toLowerCase() === name.toLowerCase())) throw new Error('garden_exists')
    const template = (body as { template?: string }).template === 'texting' ? 'k2.texting@1' : 'k2.blank@1'
    const g: Garden = { id: `g-${name.toLowerCase()}`, name, template }
    h.gardens.push(g)
    return { ok: true, garden: { ...g, index: h.gardens.length, hasFile: true } }
  }
  throw new Error(`unexpected POST ${route}`)
}

// ── Fake layout ─────────────────────────────────────────────────────────
const styleOverride = new Map<Element, Partial<{ display: string; visibility: string; opacity: number }>>()
const rectOverride = new Map<Element, ZenRect>()
const covered = new Set<Element>()
let lastRected: Element | null = null
const fakeGeometry: ZenGeometry = {
  rect(el) {
    lastRected = el
    const o = rectOverride.get(el)
    if (o) return o
    if (el.hasAttribute('data-zen-drag')) return { left: 300, top: 8, width: 500, height: 28 }
    return { left: 120, top: 8, width: 80, height: 28 }
  },
  style(el) {
    return { display: 'block', visibility: 'visible', opacity: 1, ...styleOverride.get(el) }
  },
  viewport() {
    return { width: 1200, height: 800 }
  },
  elementFromPoint() {
    if (!lastRected) return null
    return covered.has(lastRected) ? document.body : lastRected
  },
}

function setPlatform(platform: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  __resetZenAvailableForTests()
}

const unregister: Array<() => void> = []

beforeEach(() => {
  h.calls.length = 0
  h.zenHandlers.length = 0
  h.sockets.length = 0
  h.closedSockets.length = 0
  h.invokes.length = 0
  h.gardens = [{ ...DEFAULT }]
  h.setUp = true
  h.getImpl = defaultGet
  h.postImpl = defaultPost
  setPlatform('MacIntel')
  __setZenGeometryForTests(fakeGeometry)
  styleOverride.clear()
  rectOverride.clear()
  covered.clear()
  __resetZenApiForTests()
  __resetZenGardensForTests()
  localStorage.clear()
  __reloadZenWindowForTests('main')
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('agents')
})

afterEach(() => {
  cleanup()
  for (const off of unregister.splice(0)) off()
  __setZenGeometryForTests(null)
})

function mount(): void {
  render(
    <>
      <ZenTopBarToggle />
      <ZenHost />
    </>,
  )
}

function zenRoot(): HTMLElement | null {
  return document.querySelector('[data-zen-root]')
}

async function enterViaTopBar(shiftKey = false): Promise<void> {
  const toggle = document.querySelector('[data-zen-enter]')
  if (!toggle) throw new Error('no Zen toggle in the top bar')
  await act(async () => {
    fireEvent.click(toggle, { shiftKey })
  })
}

async function pageReady(template = 'k2.texting@1'): Promise<void> {
  await waitFor(() => {
    if (!document.querySelector(`[data-zen-page="${template}"]`)) throw new Error(`Zen page ${template} not drawn`)
  })
}

function safeReason(): string {
  const el = document.querySelector('[data-zen-safe-reason]')
  if (!el) throw new Error(`not in safe mode\n${document.body.innerHTML.slice(0, 2000)}`)
  return el.textContent ?? ''
}

function el(selector: string): HTMLElement {
  const found = document.querySelector(selector)
  if (!(found instanceof HTMLElement)) throw new Error(`no ${selector}`)
  return found
}

function storedWindow(label = 'main'): unknown {
  return JSON.parse(localStorage.getItem(zenWindowKey(label)) ?? 'null')
}

async function zenChanged(): Promise<void> {
  const local = h.zenHandlers.find((z) => z.hostKey === 'local')
  if (!local) throw new Error('no local zen_changed handler')
  await act(async () => local.fn())
}

describe('Zen is a mode of the window (G1–G5)', () => {
  it('the top-bar toggle turns this window’s Zen on over the page it is on, reading this computer’s daemon', async () => {
    mount()
    const toggle = el('[data-zen-enter]')
    expect(toggle.getAttribute('aria-pressed')).toBe('false')
    expect(toggle.getAttribute('title')).toBe('Zen Mode (⌃⌘Z). Hold Shift for safe mode.')
    expect(zenRoot()).toBeNull()

    await enterViaTopBar()
    await pageReady()

    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useZenWindowStore.getState().on).toBe(true)
    // The window's Garden is the first, written back (G22).
    expect(storedWindow()).toEqual({ version: 1, on: true, garden: 'g-default', view: 'home' })
    expect(el('[data-zen-garden-pill]').textContent).toContain('Garden 1')
    expect(zenRoot()?.style.zIndex).toBe('150')
    // T4.2: the list, then the Garden's page, both on the LOCAL daemon.
    const zenCalls = h.calls.filter((c) => c.route.startsWith('zen/'))
    expect(zenCalls.map((c) => [c.method, c.hostKey, c.route])).toEqual([
      ['GET', 'local', 'zen/gardens'],
      ['GET', 'local', 'zen/get'],
    ])
    expect(zenCalls[1].data).toEqual({ garden: 'g-default' })
    // Rosson 2026-10-04: the top band's usage tool reads the window's
    // server's usage, like the top bar (here this computer), and nothing else.
    expect(h.calls.filter((c) => !c.route.startsWith('zen/')).map((c) => [c.method, c.hostKey, c.route])).toEqual([
      ['GET', 'local', 'usage/subscriptions'],
    ])
    expect(h.sockets).toEqual(['local'])
    expect(h.zenHandlers.map((z) => z.hostKey)).toEqual(['local'])
  })

  it('Zen on with no folder yet sets it up once, then reads (G22)', async () => {
    h.gardens = []
    h.setUp = false
    mount()
    await enterViaTopBar()
    await pageReady()
    expect(h.calls.map((c) => [c.method, c.hostKey, c.route])).toEqual([
      ['GET', 'local', 'zen/gardens'],
      ['POST', 'local', 'zen/setup'],
      ['GET', 'local', 'zen/gardens'],
      ['GET', 'local', 'zen/get'],
    ])
    expect(h.calls[1].data).toEqual({})
    // The window opens Garden 1 (texting); Garden 2 (empty) is in the switcher.
    expect(h.calls[3].data).toEqual({ garden: 'g-default' })
    expect(el('[data-zen-garden-pill]').textContent).toContain('Garden 1')
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    expect(
      Array.from(document.querySelectorAll('[data-zen-garden-option]')).map((o) => o.textContent?.replace(/⌥⌘\d$/, '')),
    ).toEqual(['Garden 1', 'Garden 2'])
  })

  it('shows on every page and leaves that page put; exit shows it again (TG3.2)', async () => {
    mount()
    for (const page of ['agents', 'projects', 'home'] as const) {
      act(() => usePageViewStore.getState().setPage(page))
      await enterViaTopBar()
      await pageReady()
      expect([page, usePageViewStore.getState().page, zenShownNow()]).toEqual([page, page, true])
      await act(async () => void fireEvent.click(el('[data-zen-switch]')))
      expect(zenRoot()).toBeNull()
      expect(usePageViewStore.getState().page).toBe(page)
      expect(useZenWindowStore.getState().on).toBe(false)
    }
  })

  it('Settings hides Zen without turning it off; closing Settings shows it again', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => useSettingsStore.setState({ settingsOpen: true }))
    expect(zenRoot()).toBeNull()
    expect(useZenWindowStore.getState().on).toBe(true)
    act(() => useSettingsStore.setState({ settingsOpen: false }))
    await pageReady()
  })

  it('in Settings’ bar the toggle closes Settings and enters Zen', async () => {
    mount()
    act(() => useSettingsStore.setState({ settingsOpen: true }))
    await enterViaTopBar()
    await pageReady()
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
  })

  it('a page change while Zen is on (⌘P, palette, a ticket link) turns Zen off and shows that page (G34)', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => usePageViewStore.getState().setPage('projects'))
    expect(zenRoot()).toBeNull()
    expect(useZenWindowStore.getState().on).toBe(false)
    expect(usePageViewStore.getState().page).toBe('projects')
    expect(storedWindow()).toMatchObject({ on: false })
  })

  it('a new window starts outside Zen; relaunching a window keeps its own switch (G1)', async () => {
    localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: true, garden: 'g-default' }))
    act(() => __reloadZenWindowForTests('main'))
    expect(zenShownNow()).toBe(true)
    act(() => __reloadZenWindowForTests('window-2b1c'))
    expect(zenShownNow()).toBe(false)
    expect(useZenWindowStore.getState()).toMatchObject({ label: 'window-2b1c', on: false, garden: null })
    act(() => __reloadZenWindowForTests('main'))
    mount()
    await pageReady()
  })

  it('keyboard focus moves into Zen, off whatever (a terminal) had it (G50)', async () => {
    const term = document.createElement('textarea')
    term.setAttribute('data-hidden-terminal', '')
    document.body.appendChild(term)
    term.focus()
    expect(document.activeElement).toBe(term)
    mount()
    await enterViaTopBar()
    await pageReady()
    expect(document.activeElement).toBe(zenRoot())
    term.remove()
  })
})

describe('Gardens (G22–G25)', () => {
  it('the Garden switcher lists every Garden with ⌥⌘N, and a pick switches this window and reads that page', async () => {
    h.gardens = [{ ...DEFAULT }, { ...MORNINGS }]
    mount()
    await enterViaTopBar()
    await pageReady()
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    const options = Array.from(document.querySelectorAll('[data-zen-garden-option]'))
    expect(options.map((o) => [o.getAttribute('data-zen-garden-option'), o.getAttribute('data-zen-bound')])).toEqual([
      ['g-default', 'garden-option'],
      ['g-mornings', 'garden-option'],
    ])
    expect(options[1].textContent).toContain('⌥⌘2')
    expect(el('[data-zen-new-garden]').textContent).toContain('New Garden')
    await act(async () => void fireEvent.click(options[1]))
    await pageReady('k2.blank@1')
    expect(useZenWindowStore.getState().garden).toBe('g-mornings')
    expect(storedWindow()).toEqual({ version: 1, on: true, garden: 'g-mornings', view: 'home' })
    expect(h.calls.filter((c) => c.route === 'zen/get').map((c) => c.data)).toEqual([
      { garden: 'g-default' },
      { garden: 'g-mornings' },
    ])
    expect(document.querySelector('[data-zen-widget="garden-empty"]')).not.toBeNull()
    expect(document.querySelector('[data-zen-garden-menu]')).toBeNull()
  })

  it('+ New Garden asks how it starts: Start with the default makes Garden 1’s page, Start empty makes an empty Garden (TG4.2)', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden]')))
    const input = el('[data-zen-new-garden-name]') as HTMLInputElement
    expect(document.activeElement).toBe(input)
    await act(async () => void fireEvent.change(input, { target: { value: 'Notes' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    // After the name, a clear choice; nothing is made yet.
    expect(h.calls.some((c) => c.route === 'zen/garden/new')).toBe(false)
    expect(el('[data-zen-new-garden-named]').textContent).toBe('“Notes”')
    const choices = Array.from(document.querySelectorAll('[data-zen-new-garden-choice]'))
    expect(choices.map((c) => [c.getAttribute('data-zen-new-garden-choice'), c.querySelector('span')?.textContent])).toEqual([
      ['texting', 'Start with the default'],
      ['blank', 'Start empty and ask my agent'],
    ])
    // The default is the selected choice: focused, so Enter takes it.
    expect(choices[0].hasAttribute('data-zen-new-garden-selected')).toBe(true)
    expect(choices[1].hasAttribute('data-zen-new-garden-selected')).toBe(false)
    expect(document.activeElement).toBe(choices[0])
    for (const c of choices) expect(c.classList.contains('cursor-pointer')).toBe(true)
    // ↓ moves to the other choice; Esc goes back to the name, kept.
    await act(async () => void fireEvent.keyDown(choices[0], { key: 'ArrowDown' }))
    expect(document.activeElement).toBe(choices[1])
    await act(async () => void fireEvent.keyDown(choices[1], { key: 'Escape' }))
    expect((el('[data-zen-new-garden-name]') as HTMLInputElement).value).toBe('Notes')
    expect(el('[data-zen-garden-menu]')).not.toBeNull()
    await act(async () => void fireEvent.keyDown(el('[data-zen-new-garden-name]'), { key: 'Enter' }))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden-choice="texting"]')))
    await waitFor(() => expect(useZenWindowStore.getState().garden).toBe('g-notes'))
    await pageReady('k2.texting@1')
    expect(h.calls.filter((c) => c.route === 'zen/garden/new').map((c) => [c.method, c.hostKey, c.data])).toEqual([
      ['POST', 'local', { name: 'Notes', template: 'texting' }],
    ])
    expect(h.calls.filter((c) => c.route === 'zen/get').map((c) => c.data)).toEqual([
      { garden: 'g-default' },
      { garden: 'g-notes' },
    ])
    expect(document.querySelector('[data-zen-garden-menu]')).toBeNull()
    expect(el('[data-zen-garden-pill]').textContent).toContain('Notes')

    await zenChanged()
    await waitFor(() => expect(useZenGardensStore.getState().gardens.map((g) => g.id)).toEqual(['g-default', 'g-notes']))
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    expect(
      Array.from(document.querySelectorAll('[data-zen-garden-option]')).map((o) => o.getAttribute('data-zen-garden-option')),
    ).toEqual(['g-default', 'g-notes'])

    // Start empty and ask my agent: an empty Garden.
    await act(async () => void fireEvent.click(el('[data-zen-new-garden]')))
    await act(async () => void fireEvent.change(el('[data-zen-new-garden-name]'), { target: { value: 'Ideas' } }))
    await act(async () => void fireEvent.keyDown(el('[data-zen-new-garden-name]'), { key: 'Enter' }))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden-choice="blank"]')))
    await pageReady('k2.blank@1')
    expect(useZenWindowStore.getState().garden).toBe('g-ideas')
    expect(h.calls.filter((c) => c.route === 'zen/garden/new').map((c) => c.data)).toEqual([
      { name: 'Notes', template: 'texting' },
      { name: 'Ideas', template: 'blank' },
    ])
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()
  })

  it('a name you already have shows the clash copy (checked here, and from the daemon’s 409)', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden]')))
    const input = el('[data-zen-new-garden-name]') as HTMLInputElement
    await act(async () => void fireEvent.change(input, { target: { value: 'garden 1' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    expect(el('[data-zen-new-garden-error]').textContent).toBe('You already have a Garden called “garden 1”.')
    expect(document.querySelector('[data-zen-new-garden-start]')).toBeNull()
    await act(async () => void fireEvent.change(input, { target: { value: '   ' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    expect(el('[data-zen-new-garden-error]').textContent).toBe('A Garden name is 1 to 60 characters.')
    expect(h.calls.some((c) => c.route === 'zen/garden/new')).toBe(false)
    // Another window made "Later" a moment ago: the daemon refuses, and the
    // name field comes back with the reason.
    h.gardens.push({ id: 'g-later', name: 'Later', template: 'k2.blank@1' })
    await act(async () => void fireEvent.change(input, { target: { value: 'Later' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden-choice="texting"]')))
    await waitFor(() => expect(el('[data-zen-new-garden-error]').textContent).toBe('You already have a Garden called “Later”.'))
    expect((el('[data-zen-new-garden-name]') as HTMLInputElement).value).toBe('Later')
    expect(h.calls.filter((c) => c.route === 'zen/garden/new').length).toBe(1)
    expect(useZenWindowStore.getState().garden).toBe('g-default')
    // Esc cancels the field.
    await act(async () => void fireEvent.keyDown(el('[data-zen-new-garden-name]'), { key: 'Escape' }))
    expect(document.querySelector('[data-zen-new-garden-name]')).toBeNull()
  })

  it('a zen_changed that drops this window’s Garden moves it to the first one (TG4.5)', async () => {
    h.gardens = [{ ...DEFAULT }, { ...MORNINGS }]
    localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: false, garden: 'g-mornings' }))
    act(() => __reloadZenWindowForTests('main'))
    mount()
    await enterViaTopBar()
    await pageReady('k2.blank@1')
    h.gardens = [{ ...DEFAULT }]
    await zenChanged()
    await pageReady('k2.texting@1')
    expect(useZenWindowStore.getState().garden).toBe('g-default')
    expect(storedWindow()).toEqual({ version: 1, on: true, garden: 'g-default', view: 'home' })
  })
})

describe('safe mode', () => {
  it('Shift while toggling enters safe mode and reads none of the user’s files', async () => {
    mount()
    await enterViaTopBar(true)
    await pageReady()
    expect(safeReason()).toBe('You held Shift while turning Zen on.')
    expect(document.querySelector('[data-zen-safe-banner]')?.textContent).toContain(
      'Zen is in safe mode. Your files are untouched.',
    )
    expect(h.calls.filter((c) => c.route.startsWith('zen/'))).toEqual([])
    // Try again reads the list and the page.
    await act(async () => void fireEvent.click(el('[data-zen-try-again]')))
    await waitFor(() => expect(document.querySelector('[data-zen-safe-banner]')).toBeNull())
    expect(h.calls.map((c) => c.route)).toEqual(['zen/gardens', 'zen/get'])
  })

  it('a widget that crashes drops to safe mode with the message, and Exit Zen still works', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    unregister.push(
      registerZenWidget('conversation', () => {
        throw new Error('boom')
      }),
    )
    mount()
    await enterViaTopBar()
    await waitFor(() => expect(safeReason()).toBe('The page crashed: boom'))
    expect(document.querySelector('[data-zen-last-resort]')).not.toBeNull()
    act(() => void window.dispatchEvent(new Event('menu:zen-toggle')))
    expect(zenRoot()).toBeNull()
    expect(useZenWindowStore.getState().on).toBe(false)
    err.mockRestore()
  })

  it('a crash in a user page falls back to the built-in page with K2’s banner', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    unregister.push(
      registerZenWidget('agents', () => {
        if (useZenViewStore.getState().safe === null) throw new Error('agents widget broke')
        return <div data-ok-agents="" />
      }),
    )
    mount()
    await enterViaTopBar()
    await waitFor(() => expect(safeReason()).toBe('The page crashed: agents widget broke'))
    expect(document.querySelector('[data-zen-page="k2.texting@1"]')).not.toBeNull()
    expect(document.querySelector('[data-ok-agents]')).not.toBeNull()
    // Safe mode's page has its own Garden switcher.
    expect(document.querySelector('[data-zen-garden-pill]')).not.toBeNull()
    expect(document.querySelector('[data-zen-last-resort]')).toBeNull()
    act(() => void fireEvent.click(el('[data-zen-safe-exit]')))
    expect(zenRoot()).toBeNull()
    err.mockRestore()
  })

  function ControlsWithout({ omit }: { omit: 'garden-switcher' | 'zen-toggle' }): (p: ZenTemplateControlsProps) => React.JSX.Element {
    return function Controls({ bridge }: ZenTemplateControlsProps): React.JSX.Element {
      const toggle = useZenBind(bridge, 'zen-toggle')
      const trigger = useZenBind(bridge, 'garden-switcher')
      const drag = useZenBind(bridge, 'drag-region')
      return (
        <div>
          {omit !== 'garden-switcher' && <button ref={trigger} data-test-trigger="">Gardens</button>}
          <div ref={drag} data-zen-drag="" />
          {omit !== 'zen-toggle' && <button ref={toggle} data-zen-switch="">Zen</button>}
        </div>
      )
    }
  }

  it('a missing Garden switcher: one failed check is not enough, two are safe mode', async () => {
    // Custom controls draw everything (no footer) but the switcher.
    unregister.push(registerZenTemplateControls('k2.texting@1', ControlsWithout({ omit: 'garden-switcher' })))
    mount()
    await enterViaTopBar()
    await pageReady()
    // The schedule's own first-paint check may already have run once.
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t on the page.')
  })

  it('a single failed check (a mid-animation frame) does not trip safe mode', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    const toggle = el('[data-zen-switch]')
    styleOverride.set(toggle, { opacity: 0 })
    act(() => runZenControlChecksNow())
    styleOverride.delete(toggle)
    act(() => runZenControlChecksNow())
    styleOverride.set(toggle, { opacity: 0 })
    act(() => runZenControlChecksNow())
    expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Zen toggle isn’t visible.')
  })

  it('an invisible Garden switcher (opacity 0) is safe mode', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    styleOverride.set(el('[data-zen-garden-pill]'), { opacity: 0 })
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t visible.')
  })

  it('a Garden switcher covered by another element is safe mode', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    covered.add(el('[data-zen-garden-pill]'))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t visible.')
  })

  it('a Garden switcher under the stoplights is safe mode', async () => {
    mount()
    await enterViaTopBar()
    await pageReady()
    rectOverride.set(el('[data-zen-garden-pill]'), { left: 8, top: 6, width: 60, height: 24 })
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t visible.')
  })

  it('a switcher whose options miss one Garden within 1 s is “not wired”', async () => {
    h.gardens = [{ ...DEFAULT }, { ...MORNINGS }]
    unregister.push(
      registerZenTemplateControls(
        'k2.texting@1',
        function OneOption({ bridge }: ZenTemplateControlsProps) {
          const trigger = useZenBind(bridge, 'garden-switcher')
          const only = useZenBind(bridge, 'garden-option', 'g-default')
          const drag = useZenBind(bridge, 'drag-region')
          const toggle = useZenBind(bridge, 'zen-toggle')
          return (
            <div>
              <button ref={trigger} data-test-trigger="">Gardens</button>
              <button ref={only}>Garden 1</button>
              <div ref={drag} data-zen-drag="" />
              <button ref={toggle} data-zen-switch="">Zen</button>
            </div>
          )
        },
      ),
    )
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    // Shown, not yet activated: fine.
    expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
    act(() => void fireEvent.click(el('[data-test-trigger]')))
    await act(async () => {
      await new Promise((r) => setTimeout(r, ZEN_WIRING_DEADLINE_MS + 50))
    })
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t wired.')
  }, 10_000)

  it('always-shown Garden options that miss one Garden are “not wired”', async () => {
    h.gardens = [{ ...DEFAULT }, { ...MORNINGS }]
    unregister.push(
      registerZenTemplateControls(
        'k2.texting@1',
        function AlwaysShown({ bridge }: ZenTemplateControlsProps) {
          const only = useZenBind(bridge, 'garden-option', 'g-default')
          const drag = useZenBind(bridge, 'drag-region')
          const toggle = useZenBind(bridge, 'zen-toggle')
          return (
            <div>
              <button ref={only}>Garden 1</button>
              <div ref={drag} data-zen-drag="" />
              <button ref={toggle} data-zen-switch="">Zen</button>
            </div>
          )
        },
      ),
    )
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t wired.')
  })

  it('a toggle drawn without bind (bare element) is "not wired"', async () => {
    unregister.push(
      registerZenTemplateControls(
        'k2.texting@1',
        function Bare({ bridge }: ZenTemplateControlsProps) {
          const trigger = useZenBind(bridge, 'garden-switcher')
          const drag = useZenBind(bridge, 'drag-region')
          return (
            <div>
              <button ref={trigger}>Gardens</button>
              <div ref={drag} data-zen-drag="" />
              {/* Says it is the toggle, never handed to bind. */}
              <button data-zen-control="zen-toggle" onClick={() => undefined}>
                Zen
              </button>
            </div>
          )
        },
      ),
    )
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Zen toggle isn’t wired.')
  })

  it('a page that does not declare the Garden switcher is safe mode', async () => {
    h.getImpl = (route, params) => {
      const out = defaultGet(route, params)
      if (route === 'zen/get') (out as { page: { controls: string[] } }).page.controls = ['zen-toggle', 'drag-region']
      return out
    }
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Garden switcher isn’t declared by the page.')
  })

  it('both built-in templates pass the check', async () => {
    h.gardens = [{ ...DEFAULT }, { ...MORNINGS }]
    mount()
    await enterViaTopBar()
    await pageReady()
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el('[data-zen-garden-option="g-mornings"]')))
    await pageReady('k2.blank@1')
    // The blank template: the Zen toggle top right, no Add agent (no Agents widget).
    expect(el('[data-zen-top-right]').querySelector('[data-zen-switch]')).not.toBeNull()
    expect(document.querySelector('[data-zen-add-agent]')).toBeNull()
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()
  })

  it('the local daemon not answering is safe mode: “Can’t reach K2 on this computer.”', async () => {
    h.getImpl = () => {
      throw new Error('connection refused')
    }
    mount()
    await enterViaTopBar()
    await waitFor(() => expect(safeReason()).toBe('Can’t reach K2 on this computer.'))
  })

  it('a daemon without Gardens routes says to update it (G19)', async () => {
    h.getImpl = () => {
      throw new Error('unknown zen route')
    }
    mount()
    await enterViaTopBar()
    await waitFor(() =>
      expect(safeReason()).toBe('K2 on this computer is older than this app. Update it to use Gardens.'),
    )
  })

  it('a zen_changed from this computer ends safe mode and re-reads', async () => {
    mount()
    await enterViaTopBar(true)
    await pageReady()
    expect(safeReason()).toBe('You held Shift while turning Zen on.')
    await zenChanged()
    await waitFor(() => expect(document.querySelector('[data-zen-safe-banner]')).toBeNull())
    expect(h.calls.map((c) => c.route)).toEqual(['zen/gardens', 'zen/get'])
  })

  it('a config error shows the line and keeps the last good page', async () => {
    h.getImpl = (route, params) => {
      const out = defaultGet(route, params)
      if (route === 'zen/get') {
        ;(out as { errors: unknown[] }).errors = [
          { file: 'gardens/g-default.toml', line: 7, col: 3, message: "unknown color 'acent'" },
        ]
      }
      return out
    }
    mount()
    await enterViaTopBar()
    await pageReady()
    expect(document.querySelector('[data-zen-config-error]')?.textContent).toBe(
      "gardens/g-default.toml line 7: unknown color 'acent'. Showing your last good version.",
    )
    expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
  })
})

describe('the escape hatch', () => {
  it('macOS: the native menu’s event (targeted at this window) enters and exits; no webview chord; the page never changes', async () => {
    mount()
    await waitFor(() => expect(h.tauriListens.some((l) => l.event === 'menu:zen-toggle')).toBe(true))
    const menu = h.tauriListens.find((l) => l.event === 'menu:zen-toggle')
    if (!menu) throw new Error('no menu listener')
    // Only this window's label: Zen is per window.
    expect(menu.options).toEqual({ target: { kind: 'AnyLabel', label: 'main' } })

    act(() => menu.handler())
    await pageReady()
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(usePageViewStore.getState().page).toBe('agents')
    // ⌃⌘Z typed into the webview does nothing on macOS (the accelerator owns it).
    act(() => {
      fireEvent.keyDown(window, { code: 'KeyZ', key: 'z', ctrlKey: true, metaKey: true })
    })
    expect(zenRoot()).not.toBeNull()
    act(() => menu.handler())
    expect(zenRoot()).toBeNull()
    expect(useZenWindowStore.getState().on).toBe(false)
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(h.invokes.filter((i) => i.cmd === 'set_zen_menu_label').map((i) => i.args)).toEqual([
      { inZen: false },
      { inZen: true },
      { inZen: false },
    ])
  })

  it('Linux: Ctrl+Alt+Z exits even when a widget stops the key in the bubble phase; AltGr+Z does nothing', async () => {
    setPlatform('Linux x86_64')
    unregister.push(
      registerZenWidget('conversation', () => <textarea data-swallow="" onKeyDown={(e) => e.stopPropagation()} />),
    )
    mount()
    await enterViaTopBar()
    await pageReady()
    expect(document.querySelector('[data-zen-chrome-cluster="left"]')).not.toBeNull()
    const box = el('[data-swallow]')
    act(() => {
      const ev = new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true, bubbles: true })
      Object.defineProperty(ev, 'getModifierState', { value: (k: string) => k === 'AltGraph' })
      box.dispatchEvent(ev)
    })
    expect(zenRoot()).not.toBeNull()
    act(() => {
      box.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true, bubbles: true }))
    })
    expect(zenRoot()).toBeNull()
    expect(useZenWindowStore.getState().on).toBe(false)
    // From outside Zen it turns Zen on, over the page you are on.
    act(() => usePageViewStore.getState().setPage('projects'))
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true }))
    })
    await pageReady()
    expect(usePageViewStore.getState().page).toBe('projects')
  })

  it('Linux: the app menu in Zen has Exit Zen Mode, and it works in safe mode', async () => {
    setPlatform('Linux x86_64')
    mount()
    await enterViaTopBar(true)
    await pageReady()
    act(() => void fireEvent.click(el('[data-zen-app-menu]')))
    const item = screen.getByRole('menuitem', { name: 'Exit Zen Mode' })
    await act(async () => void fireEvent.click(item))
    expect(zenRoot()).toBeNull()
  })
})

// Rosson 2026-10-04: the ensō is the top bar's way in; inside Zen the top
// band keeps the "Zen" label with its switch (no icon).
describe('the Zen toggles after the icon pick', () => {
  it('top bar: one ensō toggle; Zen: the "Zen" label and switch, bound, no icon, no safe mode', async () => {
    mount()
    const enter = Array.from(document.querySelectorAll('[data-zen-enter]'))
    expect(enter.length).toBe(1)
    expect(enter[0].querySelector('svg')?.getAttribute('data-zen-icon')).toBe('enso')
    expect(enter[0].getAttribute('title')).toBe('Zen Mode (⌃⌘Z). Hold Shift for safe mode.')
    expect(enter[0].classList.contains('cursor-pointer')).toBe(true)
    await enterViaTopBar()
    await pageReady()
    const toggles = Array.from(document.querySelectorAll('[data-zen-switch]'))
    expect(toggles.length).toBe(1)
    const toggle = toggles[0]
    expect(toggle.parentElement?.hasAttribute('data-zen-top-right')).toBe(true)
    expect(toggle.getAttribute('data-zen-bound')).toBe('zen-toggle')
    expect([toggle.getAttribute('role'), toggle.getAttribute('aria-checked'), toggle.getAttribute('title')]).toEqual([
      'switch',
      'true',
      'Exit Zen Mode',
    ])
    expect(toggle.classList.contains('cursor-pointer')).toBe(true)
    expect(toggle.textContent).toBe('Zen')
    // The knob: a track with its thumb.
    const track = toggle.querySelector('span[aria-hidden]')
    expect(track?.children.length).toBe(1)
    expect(toggle.querySelector('svg, [data-zen-icon]')).toBeNull()
    expect(document.querySelector('[data-zen-root] [data-zen-icon]')).toBeNull()
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()
    expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
    await act(async () => void fireEvent.click(toggle))
    expect(useZenWindowStore.getState().on).toBe(false)
    expect(zenRoot()).toBeNull()
  })

  it('Shift on the ensō toggle still starts safe mode', async () => {
    mount()
    await enterViaTopBar(true)
    expect(useZenViewStore.getState().safe).toEqual({ kind: 'shift' })
  })
})
