// @vitest-environment jsdom
//
// 0.45.2 polish (Rosson 2026-10-08, the Diary Lab Garden): a custom widget
// that fills a full-canvas Garden has no floating ⋯; its Reload is
// "Reload <name>" in the Garden's own menu. With no menu the ⋯ stays; a
// normal column Garden is unchanged; K2's built-ins never get a Reload.
// Through the real ZenPage, chrome registry, ZenMenu and ZenCustomWidget;
// only the daemon and Tauri are faked. Fixtures are made up (g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', startDragging: async () => undefined, listen: async () => () => undefined }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params?: { widget?: string }) => {
    if (route !== 'zen/widget/bundle') throw new Error(`unexpected GET ${route}`)
    return {
      ok: true,
      widget: params?.widget,
      hash: 'hash-1',
      nonce: 'q83vFzPq1N3V0aZ8k2LmTw',
      html: '<!doctype html><html><body><script nonce="q83vFzPq1N3V0aZ8k2LmTw">k2.ready()</script></body></html>',
      bytes: 120,
    }
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { __setZenGeometryForTests } from '@/lib/zen/zen-monitor'
import { parseZenGet, ZEN_CUSTOM_KIND, type ZenResolvedPage } from '@/lib/zen/zen-page'
import { zenFillReload } from '@/lib/zen/zen-fill-reload'
import { __resetZenCustomRunForTests, useZenCustomRunStore } from '@/lib/zen/zen-custom-run'
import { resetZenPausedStartForTests, takeZenPausedAtBoot } from '@/lib/zen/zen-widgets-running'
import { registerZenWidget } from './zen-registry'
import { ZenCustomWidget } from './widgets/ZenCustomWidget'
import { ZenPage } from './ZenPage'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const GARDEN = 'g-test0001'

function custom(id: string, widget: string, name: string, column = 0): Record<string, unknown> {
  return {
    id,
    kind: 'custom',
    slot: 'column',
    column,
    props: {},
    widget,
    name,
    caps: ['agents:read'],
    requested: ['agents:read'],
    source: 'user',
    description: null,
    hash: 'hash-1',
    state: 'ok',
    errors: [],
    warnings: [],
    origin: 'local',
    paused: null,
    libs: [],
  }
}

const SWITCHER_IN = (menu: string) => ({ id: 'garden-switcher', kind: 'garden-switcher', slot: 'menu', menu, props: {}, caps: ['gardens:manage'] })
const TOGGLE_IN = (menu: string) => ({ id: 'zen-toggle', kind: 'zen-toggle', slot: 'menu', menu, props: {}, caps: [] })
const MENU = (id: string, slot = 'top') => ({ id, kind: 'menu', slot, align: 'end', props: { icon: 'dots', label: 'Lab' }, caps: [] })

/** The Diary Lab's shape: one ⋯ menu top right holding the switcher and toggle. */
function withMenu(): { items: unknown[]; bands: unknown; menus: Record<string, string[]> } {
  return {
    items: [MENU('more'), SWITCHER_IN('more'), TOGGLE_IN('more')],
    bands: { top: { start: [], center: [], end: ['more'] }, bottom: null },
    menus: { more: ['garden-switcher', 'zen-toggle'] },
  }
}

/** No menu: the switcher and toggle sit right in the top band. */
function noMenu(): { items: unknown[]; bands: unknown; menus: Record<string, string[]> } {
  return {
    items: [
      { id: 'garden-switcher', kind: 'garden-switcher', slot: 'top', align: 'start', props: {}, caps: ['gardens:manage'] },
      { id: 'zen-toggle', kind: 'zen-toggle', slot: 'top', align: 'end', props: {}, caps: [] },
    ],
    bands: { top: { start: ['garden-switcher'], center: [], end: ['zen-toggle'] }, bottom: null },
    menus: {},
  }
}

function page(opts: {
  full: boolean
  split?: number[]
  widgets: unknown[]
  chrome: { items: unknown[]; bands: unknown; menus: Record<string, string[]> }
}): ZenResolvedPage {
  const split = opts.split ?? [100]
  return parseZenGet({
    ok: true,
    schema: 1,
    version: 'v1',
    garden: { id: GARDEN, name: 'Diary Lab', index: 1 },
    page: {
      template: 'k2.blank@1',
      layout: { kind: 'columns', split, minWidths: split.map(() => 0), ...(opts.full ? { canvas: 'full' } : {}) },
      widgets: opts.widgets,
      controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
      chrome: { from: 'garden', items: opts.chrome.items },
      bands: opts.chrome.bands,
      edges: [],
      menus: opts.chrome.menus,
    },
    theme: { name: 'basic' },
    themes: [],
    chrome: {},
    motion: {},
    errors: [],
    warnings: [],
    lastGoodAt: null,
  })
}

async function show(p: ZenResolvedPage): Promise<void> {
  render(
    <div data-zen-root="">
      <ZenPage page={p} safe={false} banner={null} onControlFailure={() => undefined} />
    </div>,
  )
  // Let each frame's bundle load settle.
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

function q(sel: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(sel)
  if (!el) throw new Error(`missing ${sel}\n${document.body.innerHTML.slice(0, 3000)}`)
  return el
}

function openMenu(id: string): HTMLElement {
  act(() => {
    fireEvent.click(q(`[data-zen-menu-button="${id}"]`))
  })
  return q(`[data-zen-menu-list="${id}"]`)
}

let unregister: () => void

beforeEach(() => {
  unregister = registerZenWidget(ZEN_CUSTOM_KIND, ZenCustomWidget)
  useZenGardensStore.setState({ gardens: [{ id: GARDEN, name: 'Diary Lab', template: 'k2.blank@1', index: 1 } as never] })
  useZenWindowStore.setState({ on: true, garden: GARDEN, view: 'home' })
  __setZenGeometryForTests({
    rect: () => ({ left: 120, top: 60, width: 80, height: 30 }),
    style: () => ({ display: 'block', visibility: 'visible', opacity: 1 }),
    viewport: () => ({ width: 1200, height: 800 }),
    elementFromPoint: () => null,
  })
  __resetZenCustomRunForTests()
  resetZenPausedStartForTests()
  takeZenPausedAtBoot(null, { label: 'main', storage: null })
})

afterEach(() => {
  cleanup()
  unregister()
  __setZenGeometryForTests(null)
})

describe('a custom widget that fills a full-canvas Garden (0.45.2)', () => {
  it('with a menu: no corner ⋯; the menu holds "Reload <name>", and it reloads the widget', async () => {
    const p = page({ full: true, widgets: [custom('lab', 'diary-lab', 'Diary Lab')], chrome: withMenu() })
    expect(zenFillReload(p)).toEqual({ widgetId: 'lab', menuId: 'more', name: 'Diary Lab' })
    await show(p)
    q('[data-zen-custom-frame-slot="lab"]')
    expect(document.querySelector('[data-zen-custom-menu]'), 'no floating ⋯ over the page').toBeNull()

    const list = openMenu('more')
    const sections = Array.from(list.querySelectorAll('[data-zen-menu-section]')).map((s) => s.getAttribute('data-zen-menu-section'))
    expect(sections).toEqual(['garden-switcher', 'zen-toggle', 'reload'])
    const row = q('[data-zen-menu-reload="lab"]')
    expect(row.getAttribute('role')).toBe('menuitem')
    expect(row.textContent).toBe('Reload Diary Lab')

    const key = `${GARDEN}/lab`
    expect(useZenCustomRunStore.getState().generation[key] ?? 0).toBe(0)
    act(() => {
      fireEvent.click(row)
    })
    expect(useZenCustomRunStore.getState().generation[key]).toBe(1)
    expect(document.querySelector('[data-zen-menu-list="more"]'), 'a pick closes the menu').toBeNull()
  })

  it('with no menu: the corner ⋯ stays, so Reload is never unreachable', async () => {
    const p = page({ full: true, widgets: [custom('lab', 'diary-lab', 'Diary Lab')], chrome: noMenu() })
    expect(zenFillReload(p)).toBeNull()
    await show(p)
    q('[data-zen-custom-frame-slot="lab"]')
    act(() => {
      fireEvent.click(q('[data-zen-custom-menu]'))
    })
    act(() => {
      fireEvent.click(q('[data-zen-custom-menu-item="reload"]'))
    })
    expect(useZenCustomRunStore.getState().generation[`${GARDEN}/lab`]).toBe(1)
  })

  it('a full canvas with two widgets in its column is not "filled": the ⋯ stays and the menu has no Reload', async () => {
    const p = page({
      full: true,
      widgets: [custom('lab', 'diary-lab', 'Diary Lab'), custom('clock', 'clock', 'Clock')],
      chrome: withMenu(),
    })
    expect(zenFillReload(p)).toBeNull()
    await show(p)
    expect(document.querySelectorAll('[data-zen-custom-menu]').length).toBe(2)
    openMenu('more')
    expect(document.querySelector('[data-zen-menu-reload]')).toBeNull()
  })
})

describe('everything else is unchanged', () => {
  it('a normal column Garden: the corner ⋯ (Reload) as before; the menu has no Reload', async () => {
    const p = page({ full: false, widgets: [custom('lab', 'diary-lab', 'Diary Lab')], chrome: withMenu() })
    expect(zenFillReload(p)).toBeNull()
    await show(p)
    const corner = q('[data-zen-custom-menu]')
    expect(corner.getAttribute('aria-label')).toBe('Diary Lab menu')
    openMenu('more')
    expect(document.querySelector('[data-zen-menu-reload]')).toBeNull()
  })

  it('a built-in (the Diary) filling a full canvas: no Reload anywhere', async () => {
    const p = page({ full: true, widgets: [custom('diary', 'k2:diary@1', 'Diary')], chrome: withMenu() })
    expect(zenFillReload(p)).toBeNull()
    await show(p)
    q('[data-zen-custom-frame-slot="diary"]')
    expect(document.querySelector('[data-zen-custom-menu]')).toBeNull()
    openMenu('more')
    expect(document.querySelector('[data-zen-menu-reload]')).toBeNull()
  })
})
