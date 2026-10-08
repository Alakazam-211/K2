// @vitest-environment jsdom
//
// Settings → Gardens (prd-zen-gardens-v1 item 17) on the desktop client.
// Real Settings, real section, real `zen-settings` store; only the edges are
// faked: the daemon (`daemon-cli`, a small in-memory Gardens daemon), the
// app socket (`session-events`), Tauri and the opener plugin.
//
// Asserted (fail loudly):
//   - the nav item shows exactly where the Zen toggle shows (Mac yes,
//     Windows no until G-Win, Focus window no), sits between Projects and
//     Context Catalog, and hides when this computer's daemon has no Gardens
//     routes (`unknown zen route`, the renderer's `zen-gardens-v1` signal);
//   - not set up: one "Set up Gardens" button that POSTs `zen/setup`;
//   - list, rename, reorder and delete hit the LOCAL scope with the right
//     bodies; delete asks first and says a copy stays in history;
//   - + New Garden sends the chosen template (texting / blank);
//   - theme global and per-Garden (set and Follow global) bodies; theme
//     names come from `theme/list`;
//   - a `zen_changed` from the local daemon re-reads the list;
//   - refusals (409 garden_exists, last_garden) show inline.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type FakeGarden = { id: string; name: string; template: string; theme: string | null }

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  gardens: [] as FakeGarden[],
  setUp: true,
  outdated: false,
  global: 'default',
  themes: ['default', 'paper', 'midnight'],
  failNext: null as null | string,
  zenHandlers: [] as Array<{ hostKey: string; fn: () => void }>,
  sockets: [] as string[],
  reveals: [] as string[],
  label: 'main',
  /** `GET widget/grants` rows; null = the daemon has no such route. */
  grants: null as null | Array<Record<string, unknown>>,
  /** `GET templates` rows; null = the daemon has no such route. */
  templates: null as null | Array<Record<string, unknown>>,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: h.label,
    startDragging: async () => undefined,
    isMaximized: async () => false,
    maximize: async () => undefined,
    unmaximize: async () => undefined,
    minimize: async () => undefined,
    close: async () => undefined,
    listen: async () => () => undefined,
    onResized: async () => () => undefined,
    onFocusChanged: async () => () => undefined,
  }),
}))
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(async () => undefined),
  openPath: vi.fn(async () => undefined),
  revealItemInDir: vi.fn(async (p: string) => {
    h.reveals.push(p)
  }),
}))

function gardenJson(g: FakeGarden, i: number): unknown {
  return { id: g.id, name: g.name, index: i + 1, template: g.template, hasFile: true, createdAt: null, theme: g.theme }
}

function fakeGet(route: string): unknown {
  if (h.outdated && route.startsWith('zen/')) throw new Error('unknown zen route')
  if (route === 'zen/gardens') return { ok: true, setUp: h.setUp, gardens: h.setUp ? h.gardens.map(gardenJson) : [] }
  if (route === 'zen/status') return { ok: true, setUp: h.setUp, path: '/Users/r/.k2/zen', gardens: [] }
  if (route === 'zen/theme/list') {
    if (!h.setUp) throw new Error('zen_not_set_up')
    return {
      ok: true,
      active: h.global,
      scope: 'global',
      global: h.global,
      garden: null,
      gardenTheme: null,
      missing: false,
      themes: h.themes.map((name) => ({ name, builtin: true, user: false, summary: '', active: name === h.global })),
    }
  }
  if (route === 'zen/widget/grants') {
    if (h.grants === null) throw new Error('unknown zen route')
    return { ok: true, grants: h.grants }
  }
  if (route === 'zen/templates') {
    if (h.templates === null) throw new Error('unknown zen route')
    return { ok: true, templates: h.templates }
  }
  throw new Error(`unexpected GET ${route}`)
}

function findGarden(ref: string): FakeGarden {
  const g = h.gardens.find((x) => x.id === ref || x.name.toLowerCase() === ref.toLowerCase())
  if (!g) throw new Error('unknown_garden')
  return g
}

function fakePost(route: string, body: Record<string, unknown>): unknown {
  if (h.failNext) {
    const e = h.failNext
    h.failNext = null
    throw new Error(e)
  }
  switch (route) {
    case 'zen/setup':
      h.setUp = true
      if (h.gardens.length === 0) {
        h.gardens.push(
          { id: 'g-1', name: 'Garden 1', template: 'k2.texting@1', theme: null },
          { id: 'g-2', name: 'Garden 2', template: 'k2.blank@1', theme: null },
        )
      }
      return { ok: true }
    case 'zen/garden/new': {
      const name = String(body.name)
      if (h.gardens.some((g) => g.name.toLowerCase() === name.toLowerCase())) throw new Error('garden_exists')
      const template =
        body.template === 'texting' ? 'k2.texting@1' : body.template === 'diary' ? 'k2.diary@1' : 'k2.blank@1'
      h.gardens.push({ id: `g-new${h.gardens.length}`, name, template, theme: null })
      return { ok: true, garden: gardenJson(h.gardens[h.gardens.length - 1], h.gardens.length - 1) }
    }
    case 'zen/garden/rename': {
      const g = findGarden(String(body.garden))
      const name = String(body.name)
      if (h.gardens.some((x) => x !== g && x.name.toLowerCase() === name.toLowerCase())) throw new Error('garden_exists')
      g.name = name
      return { ok: true }
    }
    case 'zen/garden/reorder': {
      const g = findGarden(String(body.garden))
      h.gardens.splice(h.gardens.indexOf(g), 1)
      h.gardens.splice(Number(body.to) - 1, 0, g)
      return { ok: true }
    }
    case 'zen/garden/delete': {
      if (h.gardens.length === 1) throw new Error('last_garden')
      const g = findGarden(String(body.garden))
      h.gardens.splice(h.gardens.indexOf(g), 1)
      return { ok: true }
    }
    case 'zen/widget/revoke':
    case 'zen/widget/sending':
    case 'zen/widget/resume': {
      const i = (h.grants ?? []).findIndex((g) => g.garden === body.garden && g.placement === body.placement)
      if (i < 0) throw new Error('unknown_grant')
      if (route === 'zen/widget/revoke') h.grants?.splice(i, 1)
      else if (route === 'zen/widget/sending') (h.grants as Array<Record<string, unknown>>)[i].sending = body.on
      else (h.grants as Array<Record<string, unknown>>)[i].paused = null
      return { ok: true, changed: true }
    }
    case 'zen/theme/set': {
      if (body.clear === true) {
        findGarden(String(body.garden)).theme = null
      } else {
        const name = String(body.name)
        if (!h.themes.includes(name)) throw new Error('unknown_theme')
        if (typeof body.garden === 'string') findGarden(body.garden).theme = name
        else h.global = name
      }
      return { ok: true }
    }
  }
  throw new Error(`unexpected POST ${route}`)
}

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...mod,
    daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
      h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
      if (route.startsWith('zen/')) return fakeGet(route)
      // Everything else Settings' top bar asks for: an empty answer.
      if (route === 'feedback/waiting-count') return { count: 0 }
      return {}
    }),
    daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
      h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
      if (route.startsWith('zen/')) return fakePost(route, (body ?? {}) as Record<string, unknown>)
      return {}
    }),
  }
})
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
      return () => undefined
    },
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { useSettingsStore } from '@/stores/settings'
import { __resetZenAvailableForTests, zenAvailable } from '@/lib/zen/zen-platform'
import {
  __resetZenSettingsForTests,
  useZenSettingsStore,
  zenGardensSettingsShown,
  zenGardensSettingsShownFor,
} from '@/lib/zen/zen-settings'
import ZenTopBarToggle from '@/components/TopBar/ZenTopBarToggle'
import Settings, { settingsNav } from '../Settings'
import { ZenGardensSection } from './ZenGardensSection'
import { __resetZenTemplatesForTests, setZenCatalogBadge } from '@/lib/zen/zen-templates'
import { useHomesStore } from '@/stores/homes'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function setPlatform(platform: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  __resetZenAvailableForTests()
}

function zenCalls(): Array<[string, string, string, unknown]> {
  return h.calls.filter((c) => c.route.startsWith('zen/')).map((c) => [c.method, c.hostKey, c.route, c.data])
}

function zenPosts(): Array<[string, unknown]> {
  return h.calls.filter((c) => c.method === 'POST' && c.route.startsWith('zen/')).map((c) => [c.route, c.data])
}

function navIds(): string[] {
  return [...document.querySelectorAll('[data-settings-nav]')].map((el) => el.getAttribute('data-settings-nav') ?? '')
}

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-zen-garden-row="${id}"]`)
  if (!el) throw new Error(`no row for ${id}`)
  return el
}

function rowNames(): string[] {
  return [...document.querySelectorAll('[data-zen-garden-row]')].map((el) => el.getAttribute('data-zen-garden-row') ?? '')
}

async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

async function mountSection(): Promise<void> {
  render(<ZenGardensSection />)
  await waitFor(() => expect(useZenSettingsStore.getState().status).toBe('ready'))
  await settle()
}

async function pickDropdown(label: string, option: string): Promise<void> {
  fireEvent.click(screen.getByRole('button', { name: label }))
  const menu = await screen.findByTestId('setting-dropdown-menu')
  fireEvent.click(within(menu).getByText(option))
  await settle()
}

beforeEach(() => {
  h.calls.length = 0
  h.zenHandlers.length = 0
  h.sockets.length = 0
  h.reveals.length = 0
  h.gardens = [
    { id: 'g-1', name: 'Garden 1', template: 'k2.texting@1', theme: null },
    { id: 'g-2', name: 'Garden 2', template: 'k2.blank@1', theme: 'paper' },
    { id: 'g-3', name: 'Mornings', template: 'k2.blank@1', theme: null },
  ]
  h.setUp = true
  h.outdated = false
  h.global = 'default'
  h.themes = ['default', 'paper', 'midnight']
  h.failNext = null
  h.label = 'main'
  window.location.hash = ''
  setPlatform('MacIntel')
  __resetZenSettingsForTests()
  __resetZenTemplatesForTests()
  h.grants = null
  h.templates = null
  // A light section underneath, so only the sidebar and Gardens matter.
  useSettingsStore.setState({ settingsOpen: true, activeSection: 'keybindings' })
})

afterEach(() => {
  cleanup()
})

describe('Settings → Gardens: where it shows', () => {
  it('sits between Projects and Context Catalog in the sidebar', () => {
    const ids = settingsNav({ zenGardens: true }).flatMap((b) => (b.kind === 'item' ? [b.id] : []))
    const at = ids.indexOf('zen-gardens')
    expect(at).toBeGreaterThan(0)
    expect(ids[at - 1]).toBe('project-groups')
    expect(ids[at + 1]).toBe('context-catalog')
    const label = settingsNav({ zenGardens: true }).find((b) => b.kind === 'item' && b.id === 'zen-gardens')
    expect(label).toEqual({ kind: 'item', id: 'zen-gardens', label: 'Gardens' })
    expect(settingsNav({ zenGardens: false }).some((b) => b.kind === 'item' && b.id === 'zen-gardens')).toBe(false)
  })

  it('desktop Mac: the rendered sidebar shows Gardens right after Projects, and the Zen toggle shows too', async () => {
    render(
      <>
        <Settings />
        <ZenTopBarToggle />
      </>,
    )
    await settle()
    const ids = navIds()
    const at = ids.indexOf('zen-gardens')
    expect(at).toBeGreaterThan(0)
    expect(ids[at - 1]).toBe('project-groups')
    expect(ids[at + 1]).toBe('context-catalog')
    expect(screen.getByRole('button', { name: 'Gardens' })).toBeTruthy()
    // The same window shows the Zen toggle.
    expect(document.querySelectorAll('[data-zen-enter]').length).toBeGreaterThan(0)
    // The probe went to this computer's daemon only.
    expect(zenCalls().every(([, hostKey]) => hostKey === 'local')).toBe(true)
    expect(zenCalls().some(([m, , r]) => m === 'GET' && r === 'zen/gardens')).toBe(true)
  })

  it('matches the Zen toggle: hidden on Windows (until G-Win) and in a Focus window', async () => {
    for (const setup of [
      () => setPlatform('Win32'),
      () => {
        setPlatform('MacIntel')
        window.location.hash = '#focus=abc'
      },
    ]) {
      setup()
      h.calls.length = 0
      __resetZenSettingsForTests()
      expect(zenAvailable()).toBe(false)
      expect(zenGardensSettingsShown()).toBe(false)
      render(
        <>
          <Settings />
          <ZenTopBarToggle />
        </>,
      )
      await settle()
      expect(navIds()).not.toContain('zen-gardens')
      expect(navIds()).toContain('project-groups')
      expect(document.querySelector('[data-zen-enter]')).toBeNull()
      expect(zenCalls()).toEqual([])
      cleanup()
      window.location.hash = ''
    }
  })

  it('the predicate is the Zen toggle predicate, less a daemon without Gardens', () => {
    for (const available of [true, false]) {
      expect(zenGardensSettingsShownFor(available, null)).toBe(available)
      expect(zenGardensSettingsShownFor(available, { kind: 'unreachable', message: 'down' })).toBe(available)
      expect(zenGardensSettingsShownFor(available, { kind: 'outdated', message: 'unknown zen route' })).toBe(false)
    }
  })

  it('hidden when this computer’s daemon has no Gardens routes (zen-gardens-v1 missing)', async () => {
    h.outdated = true
    render(<Settings />)
    await waitFor(() => expect(useZenSettingsStore.getState().failure?.kind).toBe('outdated'))
    await settle()
    expect(navIds()).not.toContain('zen-gardens')
    expect(navIds()).toContain('context-catalog')
  })

  it('a deep link to Gardens where Zen does not exist falls back to General', async () => {
    setPlatform('Win32')
    useSettingsStore.setState({ activeSection: 'zen-gardens' })
    render(<Settings />)
    await settle()
    expect(useSettingsStore.getState().activeSection).toBe('general')
    expect(document.querySelector('[data-zen-gardens-section]')).toBeNull()
  })

  it('clicking the item opens the section', async () => {
    render(<Settings />)
    await settle()
    fireEvent.click(screen.getByRole('button', { name: 'Gardens' }))
    await settle()
    expect(useSettingsStore.getState().activeSection).toBe('zen-gardens')
    await waitFor(() => expect(document.querySelector('[data-zen-garden-row="g-1"]')).not.toBeNull())
  })
})

describe('Settings → Gardens: the section', () => {
  it('not set up: one "Set up Gardens" button that sets Zen up on the local daemon', async () => {
    h.setUp = false
    h.gardens = []
    await mountSection()
    expect(document.querySelector('[data-zen-gardens-setup]')).not.toBeNull()
    expect(document.querySelectorAll('[data-zen-garden-row]').length).toBe(0)
    // Never sets up by itself, and never asks for themes before setup.
    expect(zenPosts()).toEqual([])
    expect(zenCalls().some(([, , r]) => r === 'zen/theme/list')).toBe(false)
    const btn = screen.getByRole('button', { name: 'Set up Gardens' })
    expect(btn.className).toContain('cursor-pointer')
    fireEvent.click(btn)
    await waitFor(() => expect(rowNames()).toEqual(['g-1', 'g-2']))
    expect(zenPosts()).toEqual([['zen/setup', {}]])
    expect(zenCalls().every(([, hostKey]) => hostKey === 'local')).toBe(true)
  })

  it('lists Gardens in order with template label and position, and the folder', async () => {
    await mountSection()
    expect(rowNames()).toEqual(['g-1', 'g-2', 'g-3'])
    expect(row('g-1').textContent).toContain('Garden 1')
    expect(row('g-1').textContent).toContain('Default layout')
    expect(row('g-2').textContent).toContain('Empty')
    expect(within(row('g-3')).getByTitle('Position').textContent).toBe('3')
    expect(document.querySelector('[data-zen-gardens-path]')?.textContent).toBe('/Users/r/.k2/zen')
    fireEvent.click(screen.getByRole('button', { name: 'Reveal in Finder' }))
    await settle()
    expect(h.reveals).toEqual(['/Users/r/.k2/zen'])
    // Every request went to this computer's daemon (the Garden sync reads,
    // zen/sync and zen/news, are ZenGardenSyncControls' own and tested there).
    expect(zenCalls().every(([, k]) => k === 'local')).toBe(true)
    expect(zenCalls().filter(([, , r]) => r !== 'zen/sync' && r !== 'zen/news').map(([m, k, r]) => [m, k, r])).toEqual([
      ['GET', 'local', 'zen/gardens'],
      ['GET', 'local', 'zen/status'],
      ['GET', 'local', 'zen/theme/list'],
      // prd-zen-user-widgets-v2: widget permissions and the Garden catalog
      // (this daemon has neither route: both quietly absent).
      ['GET', 'local', 'zen/widget/grants'],
      ['GET', 'local', 'zen/templates'],
    ])
    expect(document.querySelector('[data-zen-settings-grants]')).toBeNull()
    expect(document.querySelector('[data-zen-settings-catalog]')).toBeNull()
    // Every clickable element is a pointer.
    for (const b of document.querySelectorAll('[data-zen-gardens-section] button')) {
      expect(b.className, b.textContent ?? '').toContain('cursor-pointer')
    }
  })

  it('rename inline: Enter posts {garden, name} to the local daemon', async () => {
    await mountSection()
    fireEvent.click(within(row('g-3')).getByRole('button', { name: 'Rename' }))
    const input = screen.getByRole('textbox', { name: 'New name for Mornings' })
    fireEvent.change(input, { target: { value: 'Evenings' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(row('g-3').textContent).toContain('Evenings'))
    expect(zenPosts()).toEqual([['zen/garden/rename', { garden: 'g-3', name: 'Evenings' }]])
    expect(h.calls.filter((c) => c.method === 'POST').every((c) => c.hostKey === 'local')).toBe(true)
  })

  it('rename onto a taken name shows the 409 inline, from the daemon too', async () => {
    await mountSection()
    // Caught before posting (case aside).
    fireEvent.click(within(row('g-3')).getByRole('button', { name: 'Rename' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'New name for Mornings' }), { target: { value: 'garden 1' } })
    fireEvent.click(within(row('g-3')).getByRole('button', { name: 'Save' }))
    await settle()
    expect(screen.getByTestId('zen-garden-error-g-3').textContent).toBe('You already have a Garden called “garden 1”.')
    expect(zenPosts()).toEqual([])
    // A clash the list didn't know about yet: the daemon's 409.
    h.failNext = 'garden_exists'
    fireEvent.change(screen.getByRole('textbox', { name: 'New name for Mornings' }), { target: { value: 'Launch' } })
    fireEvent.click(within(row('g-3')).getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(screen.getByTestId('zen-garden-error-g-3').textContent).toBe('You already have a Garden called “Launch”.'),
    )
    expect(zenPosts()).toEqual([['zen/garden/rename', { garden: 'g-3', name: 'Launch' }]])
    // Still editing, so the person can fix it.
    expect(screen.getByRole('textbox', { name: 'New name for Mornings' })).toBeTruthy()
  })

  it('move up / down posts {garden, to} (1-based); the ends are disabled', async () => {
    await mountSection()
    expect((screen.getByRole('button', { name: 'Move Garden 1 up' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Move Mornings down' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Move Mornings up' }))
    await waitFor(() => expect(rowNames()).toEqual(['g-1', 'g-3', 'g-2']))
    fireEvent.click(screen.getByRole('button', { name: 'Move Garden 1 down' }))
    await waitFor(() => expect(rowNames()).toEqual(['g-3', 'g-1', 'g-2']))
    expect(zenPosts()).toEqual([
      ['zen/garden/reorder', { garden: 'g-3', to: 2 }],
      ['zen/garden/reorder', { garden: 'g-1', to: 2 }],
    ])
  })

  it('delete asks first, says a copy stays in history, then posts {garden}', async () => {
    await mountSection()
    fireEvent.click(within(row('g-2')).getByRole('button', { name: 'Delete' }))
    const confirm = document.querySelector<HTMLElement>('[data-zen-garden-confirm="g-2"]')
    expect(confirm?.textContent).toContain('Delete “Garden 2”?')
    expect(confirm?.textContent).toContain('A copy stays in history')
    expect(zenPosts()).toEqual([])
    // Cancel posts nothing.
    fireEvent.click(within(confirm as HTMLElement).getByRole('button', { name: 'Cancel' }))
    expect(document.querySelector('[data-zen-garden-confirm="g-2"]')).toBeNull()
    fireEvent.click(within(row('g-2')).getByRole('button', { name: 'Delete' }))
    fireEvent.click(screen.getByRole('button', { name: 'Delete Garden' }))
    await waitFor(() => expect(rowNames()).toEqual(['g-1', 'g-3']))
    expect(zenPosts()).toEqual([['zen/garden/delete', { garden: 'g-2' }]])
  })

  it('deleting the last Garden shows the refusal inline', async () => {
    h.gardens = [{ id: 'g-1', name: 'Garden 1', template: 'k2.texting@1', theme: null }]
    await mountSection()
    fireEvent.click(within(row('g-1')).getByRole('button', { name: 'Delete' }))
    fireEvent.click(screen.getByRole('button', { name: 'Delete Garden' }))
    await waitFor(() =>
      expect(screen.getByTestId('zen-garden-error-g-1').textContent).toBe('That’s your last Garden. You need at least one.'),
    )
    expect(rowNames()).toEqual(['g-1'])
  })

  it('+ New Garden: "Start with the default" sends texting, "Start empty and ask my agent" sends blank', async () => {
    await mountSection()
    fireEvent.click(screen.getByRole('button', { name: '+ New Garden' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'New Garden name' }), { target: { value: 'Launch room' } })
    fireEvent.click(screen.getByRole('button', { name: 'Start with the default' }))
    await waitFor(() => expect(rowNames()).toHaveLength(4))
    expect(row(rowNames()[3]).textContent).toContain('Default layout')

    fireEvent.click(screen.getByRole('button', { name: '+ New Garden' }))
    // Left unnamed: the first free "Garden N".
    fireEvent.click(screen.getByRole('button', { name: 'Start empty and ask my agent' }))
    await waitFor(() => expect(rowNames()).toHaveLength(5))
    expect(row(rowNames()[4]).textContent).toContain('Empty')

    expect(zenPosts()).toEqual([
      ['zen/garden/new', { name: 'Launch room', template: 'texting' }],
      ['zen/garden/new', { name: 'Garden 5', template: 'blank' }],
    ])
    expect(h.calls.filter((c) => c.method === 'POST').every((c) => c.hostKey === 'local')).toBe(true)
  })

  it('+ New Garden onto a taken name shows the 409 inline', async () => {
    await mountSection()
    fireEvent.click(screen.getByRole('button', { name: '+ New Garden' }))
    h.failNext = 'garden_exists'
    fireEvent.change(screen.getByRole('textbox', { name: 'New Garden name' }), { target: { value: 'Ideas' } })
    fireEvent.click(screen.getByRole('button', { name: 'Start empty and ask my agent' }))
    await waitFor(() =>
      expect(screen.getByTestId('zen-new-garden-error').textContent).toBe('You already have a Garden called “Ideas”.'),
    )
    expect(rowNames()).toHaveLength(3)
  })

  it('theme: the global pick and a per-Garden override and Follow global, names from theme/list', async () => {
    // A theme the client has never heard of comes straight from the daemon.
    h.themes = ['basic', 'paper', 'midnight', 'sunrise']
    h.global = 'basic'
    await mountSection()

    await pickDropdown('Global Zen theme', 'sunrise')
    await waitFor(() => expect(useZenSettingsStore.getState().globalTheme).toBe('sunrise'))

    await pickDropdown('Theme for Garden 1', 'midnight')
    await waitFor(() =>
      expect(useZenSettingsStore.getState().gardens.find((g) => g.id === 'g-1')?.theme).toBe('midnight'),
    )

    await pickDropdown('Theme for Garden 2', 'Follow global (sunrise)')
    await waitFor(() => expect(useZenSettingsStore.getState().gardens.find((g) => g.id === 'g-2')?.theme).toBeNull())

    expect(zenPosts()).toEqual([
      ['zen/theme/set', { name: 'sunrise' }],
      ['zen/theme/set', { name: 'midnight', garden: 'g-1' }],
      ['zen/theme/set', { garden: 'g-2', clear: true }],
    ])
    expect(h.calls.filter((c) => c.method === 'POST').every((c) => c.hostKey === 'local')).toBe(true)
  })

  it('a theme refusal shows inline', async () => {
    await mountSection()
    h.failNext = 'unknown_theme'
    await pickDropdown('Global Zen theme', 'paper')
    await waitFor(() =>
      expect(screen.getByTestId('zen-gardens-theme-error').textContent).toBe('That theme is gone. Pick another one.'),
    )
  })

  it('a local zen_changed (a rename in a Zen window) re-reads the list', async () => {
    await mountSection()
    expect(h.sockets).toContain('local')
    const local = h.zenHandlers.filter((z) => z.hostKey === 'local')
    expect(local).toHaveLength(1)
    expect(h.zenHandlers.every((z) => z.hostKey === 'local')).toBe(true)
    // Renamed somewhere else (the Zen switcher, the CLI, an agent).
    h.gardens[2].name = 'Launch room'
    act(() => local[0].fn())
    await waitFor(() => expect(row('g-3').textContent).toContain('Launch room'))
  })

  it('stops listening when the section closes', async () => {
    await mountSection()
    expect(h.zenHandlers).toHaveLength(1)
    cleanup()
    expect(h.zenHandlers).toHaveLength(0)
  })

  it('a daemon that cannot be reached shows why, inline, with Try again', async () => {
    h.outdated = true
    render(<ZenGardensSection />)
    await waitFor(() => expect(screen.getByTestId('zen-gardens-load-error')).toBeTruthy())
    expect(screen.getByTestId('zen-gardens-load-error').textContent).toBe(
      'K2 on this computer is older than this app. Update it to use Gardens.',
    )
    h.outdated = false
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(rowNames()).toEqual(['g-1', 'g-2', 'g-3']))
    expect(screen.queryByTestId('zen-gardens-load-error')).toBeNull()
  })
})

describe('Settings → Gardens: the Garden catalog and widget permissions (prd-zen-user-widgets-v2)', () => {
  const STARTS = [
    { id: 'k2.texting@1', short: 'texting', label: 'Start with the default', description: 'd', section: 'start', needsGrant: null, newUsers: true },
    { id: 'k2.blank@1', short: 'blank', label: 'Start empty and ask my agent', description: 'e', section: 'start', needsGrant: null, newUsers: true },
  ]
  const DIARY = {
    id: 'k2.diary@1',
    short: 'diary',
    label: 'Diary',
    description: 'One agent at a time, in ink.',
    section: 'catalog',
    needsGrant: { widget: 'k2:diary@1', caps: ['agents:read', 'thread:read', 'thread:post'] },
    newUsers: false,
  }
  let savedHomes: ReturnType<typeof useHomesStore.getState>['homes']

  beforeEach(() => {
    savedHomes = useHomesStore.getState().homes
    useHomesStore.setState({
      homes: [{ id: 'home-work', name: 'Work', rows: [{ address: 'alice::local', workspaceId: 'w1', label: 'Alice' }] }],
      selectedId: 'home-work',
    })
  })
  afterEach(() => {
    useHomesStore.setState({ homes: savedHomes })
  })

  it('R5: the Diary is a catalog choice in + New Garden; it asks for a scope and sends the grant in the same click', async () => {
    h.templates = [...STARTS, DIARY]
    h.grants = []
    await mountSection()
    fireEvent.click(screen.getByRole('button', { name: '+ New Garden' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'New Garden name' }), { target: { value: 'Notebook' } })
    expect(document.querySelector('[data-zen-settings-catalog]')?.textContent).toContain('Ready-made Gardens')
    fireEvent.click(document.querySelector('[data-zen-settings-catalog-choice="diary"]') as HTMLElement)
    const grant = document.querySelector('[data-zen-settings-catalog-grant="diary"]') as HTMLElement
    expect(grant).not.toBeNull()
    expect(grant.textContent).toContain('See the agents in Work:')
    // Sending is on by default (R6).
    expect((grant.querySelector('[data-zen-sending-choice]') as HTMLInputElement).checked).toBe(true)
    fireEvent.click(document.querySelector('[data-zen-settings-catalog-create]') as HTMLElement)
    await waitFor(() => expect(rowNames()).toHaveLength(4))
    expect(row(rowNames()[3]).textContent).toContain('Diary')
    expect(zenPosts()).toEqual([
      ['zen/garden/new', { name: 'Notebook', template: 'diary', grant: { scope: { home: 'home-work' }, sending: true, entries: [{ server: 'local', room: 'alice' }] } }],
    ])
  })

  it('R5: nothing is ever appended; the plain starts post no grant', async () => {
    h.templates = [...STARTS, DIARY]
    h.grants = []
    await mountSection()
    expect(rowNames()).toEqual(['g-1', 'g-2', 'g-3'])
    fireEvent.click(screen.getByRole('button', { name: '+ New Garden' }))
    fireEvent.click(screen.getByRole('button', { name: 'Start with the default' }))
    await waitFor(() => expect(rowNames()).toHaveLength(4))
    expect(zenPosts()).toEqual([['zen/garden/new', { name: 'Garden 4', template: 'texting' }]])
  })

  it('Rosson 2026-10-08: the Garden catalog area: browse, preview, badge, Add makes a new Garden with its grant', async () => {
    h.templates = [...STARTS, DIARY, { ...DIARY, id: 'k2.stickers@1', short: 'stickers', label: 'Stickers', needsGrant: null }]
    h.grants = []
    h.gardens.push({ id: 'g-diary', name: 'Diary', template: 'k2.diary@1', theme: null })
    await mountSection()
    act(() => setZenCatalogBadge('diary', 'New'))
    const cards = [...document.querySelectorAll('[data-zen-catalog-card]')].map((c) => c.getAttribute('data-zen-catalog-card'))
    expect(cards).toEqual(['diary', 'stickers'])
    expect(document.querySelector('[data-zen-catalog-card="diary"] [data-zen-catalog-badge="diary"]')?.textContent).toBe('New')
    expect(document.querySelector('[data-zen-catalog-card="stickers"] [data-zen-catalog-badge]')).toBeNull()
    expect(document.querySelector('[data-zen-catalog-card="diary"] [data-zen-catalog-preview="diary"]')).not.toBeNull()
    fireEvent.click(document.querySelector('[data-zen-catalog-card="diary"]') as HTMLElement)
    const detail = document.querySelector('[data-zen-catalog-detail="diary"]') as HTMLElement
    expect(detail.textContent).toContain('One agent at a time, in ink.')
    expect(detail.textContent).toContain('Its page asks to:')
    expect(detail.querySelector('[data-zen-catalog-in-use="1"]')).not.toBeNull()
    // Nothing is created until Add, then Create.
    expect(zenPosts()).toEqual([])
    fireEvent.click(detail.querySelector('[data-zen-catalog-add]') as HTMLElement)
    expect((screen.getByRole('textbox', { name: 'Name for the new Garden' }) as HTMLInputElement).value).toBe('Diary 2')
    fireEvent.click(detail.querySelector('[data-zen-settings-catalog-create]') as HTMLElement)
    await waitFor(() => expect(rowNames()).toHaveLength(5))
    expect(zenPosts()).toEqual([
      ['zen/garden/new', { name: 'Diary 2', template: 'diary', grant: { scope: { home: 'home-work' }, sending: true, entries: [{ server: 'local', room: 'alice' }] } }],
    ])
    // A catalog Garden with no widget: Add → Create, no grant.
    fireEvent.click(document.querySelector('[data-zen-catalog-card="stickers"]') as HTMLElement)
    fireEvent.click(document.querySelector('[data-zen-catalog-add]') as HTMLElement)
    fireEvent.click(document.querySelector('[data-zen-catalog-create]') as HTMLElement)
    await waitFor(() => expect(zenPosts()).toHaveLength(2))
    expect(zenPosts()[1]).toEqual(['zen/garden/new', { name: 'Stickers', template: 'stickers' }])
  })

  it('no catalog (older daemon): no catalog area', async () => {
    await mountSection()
    expect(document.querySelector('[data-zen-settings-catalog-area]')).toBeNull()
  })

  it('UWB3: the permissions list: sending off, resume, turn off, all on the local daemon', async () => {
    h.grants = [
      {
        garden: 'g-2',
        placement: 'arcade',
        widget: 'agent-arcade',
        state: 'granted',
        caps: ['agents:read', 'thread:post'],
        scope: { home: 'home-work' },
        entries: [{ server: 'local', room: 'alice' }],
        sending: true,
        paused: { at: '2026-10-08T00:00:00Z', reason: 'runaway' },
        grantedAt: '2026-10-08T00:00:00Z',
      },
    ]
    await mountSection()
    const item = document.querySelector('[data-zen-settings-grant="g-2/arcade"]') as HTMLElement
    expect(item.textContent).toContain('agent-arcade')
    expect(item.textContent).toContain('in Garden 2')
    expect(item.textContent).toContain('Work · See agents, Post to the Thread · paused by K2')
    fireEvent.click(item.querySelector('[data-zen-settings-grant-sending="on"]') as HTMLElement)
    await waitFor(() => expect(document.querySelector('[data-zen-settings-grant-sending="off"]')).not.toBeNull())
    fireEvent.click(document.querySelector('[data-zen-settings-grant-resume]') as HTMLElement)
    await waitFor(() => expect(document.querySelector('[data-zen-settings-grant-resume]')).toBeNull())
    fireEvent.click(document.querySelector('[data-zen-settings-grant-revoke]') as HTMLElement)
    await waitFor(() => expect(document.querySelector('[data-zen-settings-grants-empty]')).not.toBeNull())
    expect(zenPosts()).toEqual([
      ['zen/widget/sending', { garden: 'g-2', placement: 'arcade', on: false, reason: 'user' }],
      ['zen/widget/resume', { garden: 'g-2', placement: 'arcade' }],
      ['zen/widget/revoke', { garden: 'g-2', placement: 'arcade' }],
    ])
    expect(h.calls.filter((c) => c.method === 'POST').every((c) => c.hostKey === 'local')).toBe(true)
  })
})
