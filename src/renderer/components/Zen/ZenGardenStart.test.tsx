// @vitest-environment jsdom
//
// Rosson 2026-10-04 — how a Garden starts: "Start with the default" on an
// empty Garden, and "Start empty and ask my agent" from + New Garden.
// Through the real ZenHost, Zen root, page, template controls, bridge and
// the real empty-Garden widget. Only the edges are faked: the daemon
// (`daemon-cli`), the app socket (`session-events`), Tauri, layout (an
// injected geometry) and the agent data verbs.
//
// Asserted (fail loudly):
//   - an empty Garden offers Start with the default next to Ask my agent;
//     a click (no confirmation: nothing is lost) asks the LOCAL daemon to
//     turn THIS Garden into `texting`; on `zen_changed` the page switches
//     live to Garden 1's layout with the same id, name and place, and no
//     safe mode follows;
//   - the button is hidden on a Garden that isn't empty (its own widgets,
//     Garden 1's page) and for a widget without `gardens:template`;
//   - a Garden whose file has changes of its own asks once, then sends
//     `force`;
//   - "Start empty and ask my agent" opens the agent chooser by itself,
//     once.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Garden = { id: string; name: string; template: string }

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  gardens: [] as Garden[],
  /** Garden ids whose file has its own changes (409 without force). */
  ownChanges: new Set<string>(),
  /** Garden ids whose page carries widgets of its own (not empty). */
  ownWidgets: new Set<string>(),
  zenHandlers: [] as Array<{ hostKey: string; fn: () => void }>,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
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

const EMPTY_CAPS = ['agents:read', 'thread:read', 'thread:post', 'gardens:template']

function pageOf(g: Garden): unknown {
  if (g.template === 'k2.blank@1') {
    const widgets: unknown[] = [
      { id: 'garden-empty', kind: 'garden-empty', column: 0, props: {}, caps: EMPTY_CAPS, source: 'builtin' },
    ]
    if (h.ownWidgets.has(g.id)) {
      widgets.push({ id: 'work', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' })
    }
    return {
      template: 'k2.blank@1',
      layout: { kind: 'columns', split: [100], minWidths: [320] },
      widgets,
      controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
    }
  }
  return {
    template: 'k2.texting@1',
    layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
    widgets: [
      { id: 'agents', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' },
      { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: ['thread:read'], source: 'builtin' },
    ],
    controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
  }
}

function listed(g: Garden, i: number): unknown {
  return { ...g, index: i + 1, hasFile: true, createdAt: '2026-10-04T18:00:00Z' }
}

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (route === 'zen/gardens') return { ok: true, setUp: true, gardens: h.gardens.map(listed) }
    if (route === 'zen/get') {
      const id = String((params as { garden?: string }).garden)
      const g = h.gardens.find((x) => x.id === id)
      if (!g) throw new Error('unknown_garden')
      return {
        ok: true,
        schema: 1,
        version: `${g.id}:${g.template}`,
        garden: { id: g.id, name: g.name, index: h.gardens.indexOf(g) + 1 },
        page: pageOf(g),
        theme: {},
        chrome: {},
        motion: {},
        errors: [],
        warnings: [],
        lastGoodAt: null,
      }
    }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    const b = (body ?? {}) as Record<string, unknown>
    if (route === 'zen/garden/new') {
      const name = String(b.name)
      const g: Garden = { id: `g-${name.toLowerCase()}`, name, template: b.template === 'texting' ? 'k2.texting@1' : 'k2.blank@1' }
      h.gardens.push(g)
      return { ok: true, garden: listed(g, h.gardens.length - 1) }
    }
    if (route === 'zen/garden/template') {
      const i = h.gardens.findIndex((x) => x.id === b.garden)
      if (i < 0) throw new Error('unknown_garden')
      if (h.ownChanges.has(String(b.garden)) && b.force !== true) throw new Error('garden_has_changes')
      const want = b.template === 'texting' ? 'k2.texting@1' : 'k2.blank@1'
      const changed = h.gardens[i].template !== want || h.ownChanges.has(String(b.garden))
      h.gardens[i] = { ...h.gardens[i], template: want }
      h.ownChanges.delete(String(b.garden))
      h.ownWidgets.delete(String(b.garden))
      return { ok: true, garden: listed(h.gardens[i], i), template: want, changed, snapshot: changed ? 'snap.toml' : null, replaced: [] }
    }
    throw new Error(`unexpected POST ${route}`)
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
    subscribeToActiveState: () => () => undefined,
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __resetZenGardensForTests, useZenGardensStore, ZEN_GARDEN_HAS_CHANGES_TEXT } from '@/lib/zen/zen-gardens'
import { __resetZenGardenAskForTests } from '@/lib/zen/zen-garden-ask'
import { __reloadZenWindowForTests, useZenWindowStore, zenWindowKey } from '@/lib/zen/zen-window'
import { __setZenGeometryForTests, runZenControlChecksNow } from '@/lib/zen/zen-monitor'
import type { ZenGeometry } from '@/lib/zen/zen-controls'
import { registerZenVerb } from '@/lib/zen/zen-bridge'
import { useZenViewStore } from '@/lib/zen/zen-view'
import { ZenHost } from './ZenHost'
import ZenTopBarToggle from '@/components/TopBar/ZenTopBarToggle'
import { registerZenWidget } from './zen-registry'
import { ZenGardenEmptyWidget, ZEN_START_DEFAULT } from './widgets/ZenGardenEmptyWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const GARDEN_1: Garden = { id: 'g-default', name: 'Garden 1', template: 'k2.texting@1' }
const GARDEN_2: Garden = { id: 'g-garden2', name: 'Garden 2', template: 'k2.blank@1' }
const LOCAL_AGENT = { address: 'cortana::local', workspaceId: 'p1', label: 'cortana' }

let lastRected: Element | null = null
const fakeGeometry: ZenGeometry = {
  rect(el) {
    lastRected = el
    if (el.hasAttribute('data-zen-drag')) return { left: 300, top: 8, width: 500, height: 28 }
    return { left: 120, top: 8, width: 80, height: 28 }
  },
  style() {
    return { display: 'block', visibility: 'visible', opacity: 1 }
  },
  viewport() {
    return { width: 1200, height: 800 }
  },
  elementFromPoint() {
    return lastRected
  },
}

const offs: Array<() => void> = []

beforeEach(() => {
  h.calls.length = 0
  h.zenHandlers.length = 0
  h.gardens = [{ ...GARDEN_1 }, { ...GARDEN_2 }]
  h.ownChanges.clear()
  h.ownWidgets.clear()
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  __setZenGeometryForTests(fakeGeometry)
  __resetZenApiForTests()
  __resetZenGardensForTests()
  __resetZenGardenAskForTests()
  localStorage.clear()
  localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: false, garden: GARDEN_2.id }))
  __reloadZenWindowForTests('main')
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('agents')
  offs.push(
    registerZenWidget('garden-empty', ZenGardenEmptyWidget),
    registerZenVerb('agents.list', () => []),
    registerZenVerb('agents.subscribe', () => () => undefined),
    registerZenVerb('agents.local', async () => [LOCAL_AGENT]),
    registerZenVerb('conversation.open', () => undefined),
    registerZenVerb('conversation.close', () => undefined),
  )
})

afterEach(() => {
  cleanup()
  for (const off of offs.splice(0)) off()
  __setZenGeometryForTests(null)
})

function el(selector: string): HTMLElement {
  const found = document.querySelector(selector)
  if (!(found instanceof HTMLElement)) throw new Error(`no ${selector}\n${document.body.innerHTML.slice(0, 1500)}`)
  return found
}

async function openZen(template = 'k2.blank@1'): Promise<void> {
  render(
    <>
      <ZenTopBarToggle />
      <ZenHost />
    </>,
  )
  await act(async () => void fireEvent.click(el('[data-zen-enter]')))
  await pageReady(template)
}

async function pageReady(template: string): Promise<void> {
  await waitFor(() => {
    if (!document.querySelector(`[data-zen-page="${template}"]`)) throw new Error(`Zen page ${template} not drawn`)
  })
}

async function zenChanged(): Promise<void> {
  const local = h.zenHandlers.find((z) => z.hostKey === 'local')
  if (!local) throw new Error('no local zen_changed handler')
  await act(async () => local.fn())
}

function templateCalls(): unknown[] {
  return h.calls.filter((c) => c.route === 'zen/garden/template').map((c) => [c.method, c.hostKey, c.data])
}

function expectNoSafeMode(): void {
  act(() => runZenControlChecksNow())
  act(() => runZenControlChecksNow())
  expect(useZenViewStore.getState().safe).toBeNull()
  expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
}

describe('Start with the default on an empty Garden', () => {
  it('sits next to Ask my agent; a click turns THIS Garden into Garden 1’s page, live, same id, name and place, no safe mode', async () => {
    await openZen()
    expect(useZenWindowStore.getState().garden).toBe(GARDEN_2.id)
    const start = el('[data-zen-start-default]')
    expect(start.textContent).toBe(ZEN_START_DEFAULT)
    expect(start.parentElement).toBe(el('[data-zen-ask-my-agent]').parentElement)
    // Everything clickable in Zen shows a pointer.
    const bare = Array.from(document.querySelectorAll('[data-zen-root] button')).filter((b) => !b.classList.contains('cursor-pointer'))
    expect(bare.map((b) => b.outerHTML.slice(0, 80))).toEqual([])
    expectNoSafeMode()

    await act(async () => void fireEvent.click(start))
    // Nothing would be lost: no confirmation.
    expect(document.querySelector('[data-zen-start-default-confirm]')).toBeNull()
    expect(templateCalls()).toEqual([['POST', 'local', { garden: GARDEN_2.id, template: 'texting' }]])
    // The daemon announces the change; the page switches live.
    await zenChanged()
    await pageReady('k2.texting@1')
    expect(document.querySelector('[data-zen-widget="garden-empty"]')).toBeNull()
    expect(useZenWindowStore.getState().garden).toBe(GARDEN_2.id)
    expect(useZenGardensStore.getState().gardens.map((g) => [g.id, g.name, g.index, g.template])).toEqual([
      [GARDEN_1.id, 'Garden 1', 1, 'k2.texting@1'],
      [GARDEN_2.id, 'Garden 2', 2, 'k2.texting@1'],
    ])
    expect(el('[data-zen-garden-pill]').textContent).toContain('Garden 2')
    expectNoSafeMode()
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1600))
    })
    expectNoSafeMode()
  }, 10_000)

  it('is hidden on a Garden that isn’t empty, on Garden 1, and for a widget without gardens:template', async () => {
    h.ownWidgets.add(GARDEN_2.id)
    await openZen()
    expect(document.querySelector('[data-zen-widget="garden-empty"]')).not.toBeNull()
    expect(document.querySelector('[data-zen-ask-my-agent]')).not.toBeNull()
    expect(document.querySelector('[data-zen-start-default]')).toBeNull()
    cleanup()

    h.ownWidgets.clear()
    localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: false, garden: GARDEN_1.id }))
    __reloadZenWindowForTests('main')
    await openZen('k2.texting@1')
    expect(document.querySelector('[data-zen-start-default]')).toBeNull()
    cleanup()

    // An older page whose empty-Garden widget lacks the cap: no button.
    const caps = EMPTY_CAPS.splice(3, 1)
    try {
      localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: false, garden: GARDEN_2.id }))
      __reloadZenWindowForTests('main')
      await openZen()
      expect(document.querySelector('[data-zen-ask-my-agent]')).not.toBeNull()
      expect(document.querySelector('[data-zen-start-default]')).toBeNull()
    } finally {
      EMPTY_CAPS.push(...caps)
    }
    expect(templateCalls()).toEqual([])
  })

  it('asks once when the Garden’s file has changes of its own, then sends force', async () => {
    h.ownChanges.add(GARDEN_2.id)
    await openZen()
    await act(async () => void fireEvent.click(el('[data-zen-start-default]')))
    await waitFor(() => expect(el('[data-zen-start-default-confirm]').textContent).toContain(ZEN_GARDEN_HAS_CHANGES_TEXT))
    expect(document.querySelector('[data-zen-page="k2.blank@1"]')).not.toBeNull()
    for (const sel of ['[data-zen-start-default-replace]', '[data-zen-start-default-cancel]']) {
      expect(el(sel).classList.contains('cursor-pointer'), sel).toBe(true)
    }
    // Cancel keeps the Garden as it is.
    await act(async () => void fireEvent.click(el('[data-zen-start-default-cancel]')))
    expect(document.querySelector('[data-zen-start-default-confirm]')).toBeNull()
    expect(templateCalls()).toEqual([['POST', 'local', { garden: GARDEN_2.id, template: 'texting' }]])
    await act(async () => void fireEvent.click(el('[data-zen-start-default]')))
    await waitFor(() => el('[data-zen-start-default-confirm]'))
    await act(async () => void fireEvent.click(el('[data-zen-start-default-replace]')))
    expect(templateCalls().slice(2)).toEqual([['POST', 'local', { garden: GARDEN_2.id, template: 'texting', force: true }]])
    await zenChanged()
    await pageReady('k2.texting@1')
    expectNoSafeMode()
  })
})

describe('+ New Garden → Start empty and ask my agent', () => {
  it('makes an empty Garden and opens the agent chooser by itself, once', async () => {
    localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: false, garden: GARDEN_1.id }))
    __reloadZenWindowForTests('main')
    await openZen('k2.texting@1')
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden]')))
    await act(async () => void fireEvent.change(el('[data-zen-new-garden-name]'), { target: { value: 'Ideas' } }))
    await act(async () => void fireEvent.keyDown(el('[data-zen-new-garden-name]'), { key: 'Enter' }))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden-choice="blank"]')))
    await pageReady('k2.blank@1')
    expect(h.calls.filter((c) => c.route === 'zen/garden/new').map((c) => c.data)).toEqual([{ name: 'Ideas', template: 'blank' }])
    await waitFor(() => expect(el('[data-zen-ask-chooser]').querySelector('[data-zen-ask-agent="cortana::local"]')).not.toBeNull())
    expectNoSafeMode()

    // Leave and come back: the empty page again, no chooser.
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el(`[data-zen-garden-option="${GARDEN_1.id}"]`)))
    await pageReady('k2.texting@1')
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el('[data-zen-garden-option="g-ideas"]')))
    await pageReady('k2.blank@1')
    expect(document.querySelector('[data-zen-ask-chooser]')).toBeNull()
    expect(document.querySelector('[data-zen-start-default]')).not.toBeNull()
  })

  it('Start with the default from + New Garden opens Garden 1’s page and no chooser', async () => {
    localStorage.setItem(zenWindowKey('main'), JSON.stringify({ version: 1, on: false, garden: GARDEN_1.id }))
    __reloadZenWindowForTests('main')
    await openZen('k2.texting@1')
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden]')))
    await act(async () => void fireEvent.change(el('[data-zen-new-garden-name]'), { target: { value: 'Desk' } }))
    await act(async () => void fireEvent.keyDown(el('[data-zen-new-garden-name]'), { key: 'Enter' }))
    await act(async () => void fireEvent.click(el('[data-zen-new-garden-choice="texting"]')))
    await waitFor(() => expect(useZenWindowStore.getState().garden).toBe('g-desk'))
    await pageReady('k2.texting@1')
    expect(h.calls.filter((c) => c.route === 'zen/garden/new').map((c) => c.data)).toEqual([{ name: 'Desk', template: 'texting' }])
    expect(document.querySelector('[data-zen-ask-chooser]')).toBeNull()
    expectNoSafeMode()
  })
})
