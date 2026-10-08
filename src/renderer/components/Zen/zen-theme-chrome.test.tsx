// @vitest-environment jsdom
//
// prd-zen-mode-v1 S5 through the real ZenHost and Zen root: the theme
// engine on the root, the native chrome held while Zen is shown (resize
// included) and restored on leave, live reload with no flicker, a bad value
// keeping the last good one, reduced motion, and the Omarchy additions
// (theme picker and cycle keys, cheat sheet, background, terminal palette).
// Only the edges are faked: the daemon, the app socket, Tauri and layout.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  page: null as unknown,
  zenHandlers: [] as Array<() => void>,
  invokes: [] as Array<{ cmd: string; args: Record<string, unknown> }>,
  tauriListens: [] as Array<{ event: string; handler: () => void; options: unknown }>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    h.invokes.push({ cmd, args: args ?? {} })
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async (event: string, handler: () => void, options?: unknown) => {
    h.tauriListens.push({ event, handler, options })
    return () => {
      const i = h.tauriListens.findIndex((l) => l.handler === handler && l.event === event)
      if (i >= 0) h.tauriListens.splice(i, 1)
    }
  }),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: 'main',
    startDragging: async () => undefined,
    isMaximized: async () => false,
    listen: async () => () => undefined,
  }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (route === 'zen/gardens') {
      return { ok: true, setUp: true, gardens: [{ id: 'g-default', name: 'Garden 1', index: 1, template: 'k2.texting@1' }] }
    }
    if (route !== 'zen/get') throw new Error(`unexpected GET ${route}`)
    return h.page
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    return { ok: true }
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...mod,
    onZenChanged: (_scope: unknown, fn: () => void) => {
      h.zenHandlers.push(fn)
      return () => {
        const i = h.zenHandlers.indexOf(fn)
        if (i >= 0) h.zenHandlers.splice(i, 1)
      }
    },
    subscribeToActiveState: () => () => undefined,
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { __resetZenGardensForTests } from '@/lib/zen/zen-gardens'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __setZenGeometryForTests } from '@/lib/zen/zen-monitor'
import type { ZenGeometry } from '@/lib/zen/zen-controls'
import { useZenViewStore } from '@/lib/zen/zen-view'
import { useZenAppliedThemeStore } from '@/lib/zen/zen-theme'
import { useZenOverlayStore } from '@/lib/zen/zen-theme-switch'
import { getChromeSource } from '@/stores/style'
import { ZenHost } from './ZenHost'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const PNG =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='

function zenPage(opts: { version?: string; accent?: string; chrome?: object; theme?: object; errors?: object[] } = {}): unknown {
  return {
    ok: true,
    schema: 1,
    version: opts.version ?? 'v1',
    page: {
      template: 'k2.texting@1',
      layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
      widgets: [
        { id: 'agents', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' },
        { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: ['thread:read'], source: 'builtin' },
      ],
      controls: ['zen-toggle', 'garden-switcher', 'drag-region'],
    },
    // The daemon's `theme` shape (store.rs `resolve`).
    theme: {
      name: 'k2-light',
      builtin: true,
      user: false,
      scope: 'global',
      tokens: {
        scheme: 'light',
        colors: { light: { accent: opts.accent ?? '#112233' }, dark: {} },
        shape: { radius: 14, 'bubble-radius': 18, gap: 12, 'list-width': 300 },
      },
      font: {
        family: 'mono',
        stack: 'ui-monospace, monospace',
        monospace: true,
        size: 14,
        lineHeight: 1.45,
        terminal: { family: 'mono', stack: 'ui-monospace, monospace', monospace: true },
      },
      terminal: { palette: { light: { red: '#aa0000', 'bright-white': '#fefefe' }, dark: {} } },
      ...opts.theme,
    },
    themes: [
      { name: 'k2-light', label: 'K2 Light', builtin: true, user: false, active: true },
      { name: 'k2-dark', label: 'K2 Dark', builtin: true, user: false, active: false },
      { name: 'mine', builtin: false, user: true, active: false },
    ],
    chrome: opts.chrome ?? { corners: 'square', stoplights: 'square', 'stoplight-offset': [6, 4] },
    motion: {},
    errors: opts.errors ?? [],
    warnings: [],
    lastGoodAt: null,
  }
}

// Every control is on screen and on top (jsdom has no layout).
let lastRected: Element | null = null
const geometry: ZenGeometry = {
  rect: (el) => {
    lastRected = el
    return el.hasAttribute('data-zen-drag')
      ? { left: 300, top: 40, width: 500, height: 28 }
      : { left: 120, top: 40, width: 80, height: 28 }
  },
  style: () => ({ display: 'block', visibility: 'visible', opacity: 1 }),
  viewport: () => ({ width: 1200, height: 800 }),
  elementFromPoint: () => lastRected,
}

function setPlatform(platform: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  __resetZenAvailableForTests()
}

function root(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-zen-root]')
  if (!el) throw new Error('Zen is not shown')
  return el
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()))
}

function lastNative(cmd: string): Record<string, unknown> {
  const hit = [...h.invokes].reverse().find((i) => i.cmd === cmd)
  if (!hit) throw new Error(`no ${cmd} call`)
  return hit.args
}

async function enterZen(): Promise<void> {
  render(<ZenHost />)
  act(() => useZenWindowStore.getState().setOn(true))
  await waitFor(() => {
    if (!document.querySelector('[data-zen-page]')) throw new Error('Zen page not drawn')
  })
}

let mediaMatches: Record<string, boolean> = {}

beforeEach(() => {
  h.calls.length = 0
  h.zenHandlers.length = 0
  h.invokes.length = 0
  h.page = zenPage()
  mediaMatches = {}
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: (query: string) => ({
      matches: mediaMatches[query] === true,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }),
  })
  setPlatform('MacIntel')
  __setZenGeometryForTests(geometry)
  __resetZenApiForTests()
  __resetZenGardensForTests()
  localStorage.clear()
  useZenWindowStore.setState({ on: false, garden: null })
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useZenOverlayStore.setState({ sheet: false, picker: false })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('home')
  // The user's Style: Glass (round lights at an 8px inset, system corners).
  document.documentElement.setAttribute('data-style', 'glass')
  document.documentElement.style.setProperty('--inset-window', '8px')
})

afterEach(() => {
  cleanup()
  __setZenGeometryForTests(null)
  setPlatform('')
})

describe('theme on the Zen root', () => {
  it('applies only --zen-* tokens from the bundle, on the root (never <html>), with keyframe presets', async () => {
    const htmlBefore = document.documentElement.getAttributeNames().map((n) => `${n}=${document.documentElement.getAttribute(n)}`)
    await enterZen()
    await waitFor(() => expect(root().style.getPropertyValue('--zen-accent')).toBe('rgb(17, 34, 51)'))
    expect(root().getAttribute('data-zen-scheme')).toBe('light')
    expect(root().style.getPropertyValue('--zen-term-red')).toBe('rgb(170, 0, 0)')
    expect(root().style.getPropertyValue('--zen-term-bright-white')).toBe('rgb(254, 254, 254)')
    expect(root().style.getPropertyValue('--zen-terminal-font-family')).toContain('monospace')
    expect(root().style.getPropertyValue('--zen-anim-rowIn-ease')).toBe('cubic-bezier(0.22, 1, 0.36, 1)')
    expect(document.querySelector('[data-zen-keyframes]')?.textContent).toContain('@keyframes zen-kf-popin')
    expect(document.documentElement.style.getPropertyValue('--zen-accent')).toBe('')
    const htmlAfter = document.documentElement.getAttributeNames().map((n) => `${n}=${document.documentElement.getAttribute(n)}`)
    expect(htmlAfter).toEqual(htmlBefore)
    // Terminals in Zen read the bundle's palette and face.
    expect(useZenAppliedThemeStore.getState().applied).toMatchObject({ name: 'k2-light', scheme: 'light' })
    expect(useZenAppliedThemeStore.getState().applied?.terminal.red).toBe('rgb(170, 0, 0)')
  })

  it('live reload on zen_changed swaps the theme with no loading flash; a bad value keeps the last good one', async () => {
    await enterZen()
    await waitFor(() => expect(root().style.getPropertyValue('--zen-accent')).toBe('rgb(17, 34, 51)'))
    const seenLoading: boolean[] = []
    const mo = new MutationObserver(() => seenLoading.push(document.querySelector('[data-zen-loading]') !== null))
    mo.observe(document.body, { childList: true, subtree: true })

    h.page = zenPage({ version: 'v2', accent: '#445566' })
    await act(async () => {
      for (const fn of [...h.zenHandlers]) fn()
    })
    await waitFor(() => expect(root().style.getPropertyValue('--zen-accent')).toBe('rgb(68, 85, 102)'))

    // A bad accent with the daemon's error: the last good accent stays, the banner names the line.
    h.page = zenPage({
      version: 'v3',
      accent: 'not-a-colour',
      errors: [{ file: 'zen.toml', line: 12, col: 3, message: "unknown color 'acent'" }],
    })
    await act(async () => {
      for (const fn of [...h.zenHandlers]) fn()
    })
    await waitFor(() => expect(document.body.textContent).toContain('zen.toml line 12'))
    expect(root().style.getPropertyValue('--zen-accent')).toBe('rgb(68, 85, 102)')
    mo.disconnect()
    expect(seenLoading.includes(true)).toBe(false)
  })

  it('reduced motion wins on the root', async () => {
    mediaMatches['(prefers-reduced-motion: reduce)'] = true
    await enterZen()
    await waitFor(() => expect(root().hasAttribute('data-zen-reduced-motion')).toBe(true))
    expect(root().style.getPropertyValue('--zen-anim-messageIn-duration')).toBe('0ms')
    expect(root().style.getPropertyValue('--zen-anim-messageIn-keyframes')).toBe('none')
  })

  it('the theme’s background image is drawn with its fit and opacity; reduced transparency drops it', async () => {
    h.page = zenPage({
      theme: {
        background: {
          dataUrl: PNG,
          mime: 'image/png',
          bytes: 68,
          file: 'background.png',
          fit: 'contain',
          opacity: 0.7,
          lastGood: false,
        },
      },
    })
    await enterZen()
    await waitFor(() => expect(document.querySelector('[data-zen-background]')).not.toBeNull())
    const bg = document.querySelector<HTMLElement>('[data-zen-background]')
    if (!bg) throw new Error('no background')
    expect(bg.getAttribute('data-zen-background-fit')).toBe('contain')
    expect(bg.style.opacity).toBe('0.7')
    expect(bg.style.backgroundSize).toBe('contain')
    expect(bg.style.backgroundRepeat).toBe('no-repeat')
    expect(bg.style.backgroundImage).toContain('data:image/png;base64,')
    cleanup()
    mediaMatches['(prefers-reduced-transparency: reduce)'] = true
    await enterZen()
    await waitFor(() => expect(root().style.getPropertyValue('--zen-accent')).toBe('rgb(17, 34, 51)'))
    expect(document.querySelector('[data-zen-background]')).toBeNull()
  })
})

describe('native chrome: held while Zen is shown, restored on leave (macOS)', () => {
  it('Rosson 2026-10-08 (the Diary): stoplights "hidden" hides the native buttons, reserves nothing, and leaving shows them again', async () => {
    h.page = zenPage({ chrome: { corners: 'system', stoplights: 'hidden', 'stoplight-offset': [0, 0] } })
    await enterZen()
    await waitFor(() => expect(lastNative('set_traffic_light_inset')).toMatchObject({ hidden: true }))
    expect(root().getAttribute('data-zen-stoplights')).toBe('hidden')
    expect(root().style.getPropertyValue('--zen-stoplight-rect')).toBe('0px 0px 0px 0px')
    expect(root().style.getPropertyValue('--zen-stoplight-safe-top')).toBe('0px')
    expect(root().style.getPropertyValue('--zen-stoplight-safe-left')).toBe('0px')
    // Every re-apply keeps them hidden.
    h.invokes.length = 0
    window.dispatchEvent(new Event('resize'))
    await nextFrame()
    const insets = h.invokes.filter((i) => i.cmd === 'set_traffic_light_inset')
    expect(insets.length).toBeGreaterThan(0)
    for (const i of insets) expect(i.args).toMatchObject({ hidden: true })
    // Leaving Zen (Settings over it) gives the window its buttons back.
    act(() => useSettingsStore.setState({ settingsOpen: true }))
    expect(lastNative('set_traffic_light_inset')).toMatchObject({ hidden: false })
    act(() => useSettingsStore.setState({ settingsOpen: false }))
  })

  it('Rosson 2026-10-08 (the Diary): [layout] canvas "full" floats the top band over a bare, edge-to-edge page', async () => {
    const full = zenPage() as { page: { layout: Record<string, unknown> } }
    full.page.layout = { ...full.page.layout, canvas: 'full' }
    h.page = full
    await enterZen()
    await waitFor(() => expect(document.querySelector('[data-zen-canvas="full"]')).not.toBeNull())
    const band = document.querySelector<HTMLElement>('[data-zen-band-float]')
    if (!band) throw new Error('no floating band')
    expect(band.style.position).toBe('absolute')
    const layout = document.querySelector<HTMLElement>('[data-zen-layout]')
    expect(layout?.style.padding).toBe('0px')
    const cols = [...document.querySelectorAll('[data-zen-column]')]
    expect(cols.length).toBeGreaterThan(0)
    for (const c of cols) expect(c.hasAttribute('data-zen-column-bare'), 'no glass box on a full canvas').toBe(true)
    // A framed page (the default) keeps its row and its boxes.
    cleanup()
    h.page = zenPage()
    await enterZen()
    await waitFor(() => expect(document.querySelector('[data-zen-layout]')).not.toBeNull())
    expect(document.querySelector('[data-zen-canvas]')).toBeNull()
    expect(document.querySelector('[data-zen-band-float]')).toBeNull()
  })

  it('enter takes Zen values; resize and fullscreen keep them; Settings and exit restore the Style', async () => {
    await enterZen()
    await waitFor(() => expect(lastNative('set_window_corner_radius')).toEqual({ radius: 0.5 }))
    expect(lastNative('set_traffic_light_inset')).toMatchObject({ x: 14, y: 15, square: true })
    expect(root().getAttribute('data-zen-corners')).toBe('square')
    expect(useZenViewStore.getState().safe).toBeNull()
    // Zen's own inset (8, 8) plus the theme's offset (6, 4): the page keeps
    // clear of the moved lights: 69 + 8 + 6 = 83 wide, 4 + 8 + 3 + 28 = 43 tall.
    expect(root().style.getPropertyValue('--zen-stoplight-rect')).toBe('0px 0px 83px 43px')
    expect(root().style.getPropertyValue('--zen-stoplight-safe-left')).toBe('97px')

    for (const fire of [
      () => window.dispatchEvent(new Event('resize')),
      () => document.dispatchEvent(new Event('fullscreenchange')),
    ]) {
      h.invokes.length = 0
      fire()
      await nextFrame()
      const insets = h.invokes.filter((i) => i.cmd === 'set_traffic_light_inset')
      const radii = h.invokes.filter((i) => i.cmd === 'set_window_corner_radius')
      expect(insets.length).toBeGreaterThan(0)
      for (const i of insets) expect(i.args).toMatchObject({ x: 14, y: 15, square: true })
      for (const r of radii) expect(r.args).toEqual({ radius: 0.5 })
    }
    // Still the page, not safe mode (which would use K2's default chrome).
    expect(useZenViewStore.getState().safe).toBeNull()

    // Settings hides Zen: the Style comes back.
    h.invokes.length = 0
    act(() => useSettingsStore.setState({ settingsOpen: true }))
    expect(lastNative('set_window_corner_radius')).toEqual({ radius: 0 })
    expect(lastNative('set_traffic_light_inset')).toMatchObject({ x: 8, y: 11, square: false })
    expect(getChromeSource()).toBe('style')
    act(() => useSettingsStore.setState({ settingsOpen: false }))
    await waitFor(() => expect(lastNative('set_window_corner_radius')).toEqual({ radius: 0.5 }))

    // Exit Zen (the page's own toggle): the Style again.
    h.invokes.length = 0
    const toggle = document.querySelector('[data-zen-switch]')
    if (!toggle) throw new Error('no Zen switch')
    act(() => void fireEvent.click(toggle))
    expect(document.querySelector('[data-zen-root]')).toBeNull()
    expect(lastNative('set_window_corner_radius')).toEqual({ radius: 0 })
    expect(lastNative('set_traffic_light_inset')).toMatchObject({ x: 8, y: 11, square: false })
    expect(useZenAppliedThemeStore.getState().applied).toBeNull()
  })

  it('a chrome change on zen_changed re-sends once; a bad chrome value keeps the last good', async () => {
    await enterZen()
    await waitFor(() => expect(lastNative('set_window_corner_radius')).toEqual({ radius: 0.5 }))
    h.invokes.length = 0
    h.page = zenPage({ version: 'v2', chrome: { corners: 'system', stoplights: 'triangle', 'stoplight-offset': [0, 0] } })
    await act(async () => {
      for (const fn of [...h.zenHandlers]) fn()
    })
    await waitFor(() => expect(lastNative('set_window_corner_radius')).toEqual({ radius: 0 }))
    // `triangle` isn't a stoplight shape: square (last good) holds.
    expect(lastNative('set_traffic_light_inset')).toMatchObject({ x: 8, y: 11, square: true })
    // Never a flip through the Style (round lights) in between.
    expect(h.invokes.filter((i) => i.cmd === 'set_traffic_light_inset').some((i) => i.args.square === false)).toBe(false)
  })
})

describe('theme picker and cycle keys (Omarchy 2)', () => {
  it('shows the active theme, switches on click through this computer’s daemon, and cycles with ⌃⌘. / ⌃⌘⇧.', async () => {
    await enterZen()
    await waitFor(() => expect(document.querySelector('[data-zen-active-theme]')?.textContent).toBe('K2 Light'))
    const button = document.querySelector('[data-zen-theme-button]')
    if (!button) throw new Error('no theme button')
    act(() => void fireEvent.click(button))
    expect(document.querySelector('[data-zen-theme-option="k2-light"]')?.getAttribute('aria-selected')).toBe('true')
    // Rosson 2026-10-04: the picker shows capitalized names (the daemon's
    // label, else the id with a capital); the id is what a switch sends.
    expect(Array.from(document.querySelectorAll('[data-zen-theme-option]')).map((b) => [b.getAttribute('data-zen-theme-option'), b.textContent])).toEqual([
      ['k2-light', '✓K2 Lightbuilt in'],
      ['k2-dark', 'K2 Darkbuilt in'],
      ['mine', 'Mine'],
    ])
    h.calls.length = 0
    const mine = document.querySelector('[data-zen-theme-option="mine"]')
    if (!mine) throw new Error('no option')
    await act(async () => void fireEvent.click(mine))
    await waitFor(() =>
      expect(h.calls.map((c) => [c.method, c.hostKey, c.route])).toEqual([
        ['POST', 'local', 'zen/theme/set'],
        ['GET', 'local', 'zen/get'],
      ]),
    )
    expect(h.calls[0].data).toEqual({ name: 'mine' })

    h.calls.length = 0
    await act(async () => {
      fireEvent.keyDown(document.body, { code: 'Period', key: '.', ctrlKey: true, metaKey: true })
    })
    await act(async () => {
      fireEvent.keyDown(document.body, { code: 'Period', key: '>', ctrlKey: true, metaKey: true, shiftKey: true })
    })
    // The daemon owns the order and the wrap: next and prev, not a computed set.
    const switches = h.calls.filter((c) => c.method === 'POST').map((c) => [c.route, c.data])
    expect(switches).toEqual([
      ['zen/theme/next', {}],
      ['zen/theme/prev', {}],
    ])
  })

  it('a Garden with its own theme pick keeps it: set, next and prev carry the Garden (G16, G30)', async () => {
    h.page = zenPage({ theme: { scope: 'garden' } })
    await enterZen()
    await waitFor(() => expect(document.querySelector('[data-zen-active-theme]')?.textContent).toBe('K2 Light'))
    const gardenId = 'g-default'
    h.calls.length = 0
    await act(async () => {
      fireEvent.keyDown(document.body, { code: 'Period', key: '.', ctrlKey: true, metaKey: true })
    })
    await act(async () => {
      fireEvent.keyDown(document.body, { code: 'Period', key: '>', ctrlKey: true, metaKey: true, shiftKey: true })
    })
    const button = document.querySelector('[data-zen-theme-button]')
    if (!button) throw new Error('no theme button')
    act(() => void fireEvent.click(button))
    const mine = document.querySelector('[data-zen-theme-option="mine"]')
    if (!mine) throw new Error('no option')
    await act(async () => void fireEvent.click(mine))
    await waitFor(() => expect(h.calls.filter((c) => c.method === 'POST')).toHaveLength(3))
    expect(h.calls.filter((c) => c.method === 'POST').map((c) => [c.route, c.data])).toEqual([
      ['zen/theme/next', { garden: gardenId }],
      ['zen/theme/prev', { garden: gardenId }],
      ['zen/theme/set', { name: 'mine', garden: gardenId }],
    ])
  })

  it('safe mode draws no picker and the keys change nothing', async () => {
    await enterZen()
    act(() => useZenViewStore.getState().enterSafeMode({ kind: 'crash', message: 'x' }))
    expect(document.querySelector('[data-zen-theme-picker]')).toBeNull()
    h.calls.length = 0
    await act(async () => {
      fireEvent.keyDown(document.body, { code: 'Period', key: '.', ctrlKey: true, metaKey: true })
    })
    expect(h.calls.filter((c) => c.method === 'POST')).toEqual([])
  })
})

describe('shortcut cheat sheet (Omarchy 4)', () => {
  it('opens on ?, ⌃⌘/, and the menus; Esc closes; ? while typing does not', async () => {
    await enterZen()
    const sheet = (): Element | null => document.querySelector('[data-zen-shortcut-sheet]')
    act(() => void fireEvent.keyDown(document.body, { key: '?', code: 'Slash', shiftKey: true }))
    expect(sheet()).not.toBeNull()
    for (const keys of ['⌃⌘Z', '⌃⌘.', '⌃⌘⇧.', '⌘1–9', '⌥⌘1–9']) {
      expect(document.querySelector(`[data-zen-shortcut="${keys}"]`), keys).not.toBeNull()
    }
    act(() => void fireEvent.keyDown(document.body, { key: 'Escape', code: 'Escape' }))
    expect(sheet()).toBeNull()

    const box = document.createElement('textarea')
    root().appendChild(box)
    act(() => void fireEvent.keyDown(box, { key: '?', code: 'Slash', shiftKey: true }))
    expect(sheet()).toBeNull()
    act(() => void fireEvent.keyDown(box, { key: '/', code: 'Slash', ctrlKey: true, metaKey: true }))
    expect(sheet()).not.toBeNull()
    act(() => void fireEvent.keyDown(document.body, { key: 'Escape', code: 'Escape' }))
    box.remove()

    // macOS View menu "Zen Shortcuts" → this window only.
    const menu = h.tauriListens.find((l) => l.event === 'menu:zen-shortcuts')
    if (!menu) throw new Error('no menu:zen-shortcuts listener')
    expect(menu.options).toEqual({ target: { kind: 'AnyLabel', label: 'main' } })
    act(() => menu.handler())
    expect(sheet()).not.toBeNull()
    act(() => void fireEvent.keyDown(document.body, { key: 'Escape', code: 'Escape' }))
    // Linux / Windows app menu item: a DOM event.
    act(() => void window.dispatchEvent(new Event('menu:zen-shortcuts')))
    expect(sheet()).not.toBeNull()
  })

  it('Linux: Ctrl+Alt+. cycles and Ctrl+Alt+/ opens the sheet; AltGr does neither', async () => {
    setPlatform('Linux x86_64')
    await enterZen()
    await waitFor(() => expect(document.querySelector('[data-zen-active-theme]')?.textContent).toBe('K2 Light'))
    const altGr = (init: KeyboardEventInit): KeyboardEvent => {
      const ev = new KeyboardEvent('keydown', { bubbles: true, ...init })
      Object.defineProperty(ev, 'getModifierState', { value: (k: string) => k === 'AltGraph' })
      return ev
    }
    h.calls.length = 0
    act(() => void document.body.dispatchEvent(altGr({ code: 'Period', key: '.', ctrlKey: true, altKey: true })))
    act(() => void document.body.dispatchEvent(altGr({ code: 'Slash', key: '/', ctrlKey: true, altKey: true })))
    expect(h.calls).toEqual([])
    expect(document.querySelector('[data-zen-shortcut-sheet]')).toBeNull()
    await act(async () => {
      fireEvent.keyDown(document.body, { code: 'Period', key: '.', ctrlKey: true, altKey: true })
    })
    expect(h.calls.filter((c) => c.method === 'POST').map((c) => [c.route, c.data])).toEqual([['zen/theme/next', {}]])
    act(() => void fireEvent.keyDown(document.body, { code: 'Slash', key: '/', ctrlKey: true, altKey: true }))
    expect(document.querySelector('[data-zen-shortcut="Ctrl+Alt+Z"]')).not.toBeNull()
    // No native chrome call on Linux.
    expect(h.invokes.filter((i) => i.cmd === 'set_window_corner_radius' || i.cmd === 'set_traffic_light_inset')).toEqual([])
  })
})
