// @vitest-environment jsdom
//
// prd-garden-access-v1 §6 (A5): Settings → Gardens with Garden access.
// The real section, the real page, the real stores; only the edges are
// faked: the daemon (`daemon-cli`, a small in-memory Gardens + access
// daemon), the app socket, Tauri and the opener plugin.
//
// Asserted (fail loudly):
//   - no switch anywhere (Rosson 2026-10-23): a 0.46.3+ daemon answers
//     gardenAccess:true and the page is the master-detail page; an older
//     daemon (no key) gets today's page, with no switch row either;
//   - the sidebar is Styles first, then each Garden (lock glyph, "N
//     editors · made by X"), + New Garden, Deleted Gardens (n), and last
//     "Not in a Garden";
//   - a Garden's pane shows its Local Data (its placed widgets' data) and
//     External Data (the sources its cards read); a widget or source in two
//     Gardens shows under both; what no Garden uses is under "Not in a
//     Garden"; delete and remove work there as on the old pages;
//   - the nav (Gardens one page between Tickets and Context Catalog) and
//     search landing (Local/External Data results land on Gardens);
//   - the Garden pane: Made by, the editors with "added automatically at
//     upgrade", Sees "Everything you can see", Lock with its caption, Get
//     K2's design updates, Widgets with their last signer; no "not signed"
//     and no Sign anywhere on the page;
//   - Remove posts `allow:false` and offers an 8 s Undo that posts
//     `allow:true`; + Add agent lists this computer's agents A–Z (a listed
//     one checked) and posts `allow:true`;
//   - History… opens the Garden modal; a widget's History… opens its modal,
//     which alone shows "not signed" and Sign (posts `zen/widget/sign`);
//   - Deleted Gardens: Restore posts `zen/garden/restore`; its History is
//     read-only (no Sign, no Restore);
//   - Widget data trims (G28): on = "Widget data", no It's mine / Let …
//     use, grants under Advanced; off = Local Data with them, as 0.46.3.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Editor = { agent: string; handle: string; name: string; color: string; addedBy: string; addedAt: number }

const NOW = 1_792_000_000 // 2026-10 (unix s)

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  /** undefined = the daemon has no switch (no key on settings/get). */
  gardenAccess: undefined as boolean | undefined,
  gardens: [] as Array<{ id: string; name: string; template: string }>,
  locked: {} as Record<string, boolean>,
  editors: {} as Record<string, Editor[]>,
  made: {} as Record<string, { agent: string; handle: string; name: string; at: number } | null>,
  deleted: [] as Array<Record<string, unknown>>,
  /** null = the daemon has no Deleted Gardens route. */
  deletedRoute: true,
  history: {} as Record<string, unknown>,
  widgetHistory: {} as Record<string, unknown>,
  agents: [] as Array<Record<string, unknown>>,
  widgets: [] as Array<Record<string, unknown>>,
  spaces: [] as Array<Record<string, unknown>>,
  /** Each Garden's resolved page (`zen/get`): custom placements. */
  pages: {} as Record<string, Array<{ widget: string; sourceAddresses?: string[] }>>,
  /** The window's server's sources (`sources/list`). */
  sources: [] as Array<Record<string, unknown>>,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: 'main',
    listen: async () => () => undefined,
    onFocusChanged: async () => () => undefined,
  }),
}))
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(async () => undefined),
  openPath: vi.fn(async () => undefined),
  revealItemInDir: vi.fn(async () => undefined),
}))

function editorCount(id: string): number {
  return (h.editors[id] ?? []).length
}

function fakeGet(route: string, p: Record<string, unknown>): unknown {
  switch (route) {
    case 'settings/get':
      return h.gardenAccess === undefined ? { theme: 'dark' } : { theme: 'dark', gardenAccess: h.gardenAccess }
    case 'zen/gardens':
      return {
        ok: true,
        setUp: true,
        gardens: h.gardens.map((g, i) => ({
          id: g.id,
          name: g.name,
          index: i + 1,
          template: g.template,
          hasFile: true,
          createdAt: null,
          theme: null,
          ...(h.gardenAccess
            ? {
                editors: editorCount(g.id),
                madeBy: h.made[g.id] ? { agent: h.made[g.id]?.agent, handle: h.made[g.id]?.handle, name: h.made[g.id]?.name } : null,
              }
            : {}),
        })),
      }
    case 'zen/status':
      return { ok: true, setUp: true, path: '/Users/r/.k2/zen', gardens: [] }
    case 'zen/theme/list':
      return {
        ok: true,
        global: 'default',
        themes: [
          { name: 'default', builtin: true, summary: 'K2’s look' },
          { name: 'paper', builtin: true, summary: '' },
        ],
      }
    case 'zen/templates':
      return { ok: true, templates: [] }
    case 'zen/sync':
      return {
        ok: true,
        liveDefaults: 'd-1111222233334444',
        k2Version: '0.46.3',
        previousDefaults: null,
        gardens: h.gardens.map((g, i) => ({
          id: g.id,
          name: g.name,
          index: i + 1,
          page: { mode: 'synced', newerDefault: false },
          theme: { mode: 'synced', newerDefault: false },
          ownChanges: [],
          undo: null,
          keepPrevious: { page: true, theme: true },
          themeName: 'basic',
          themeLabel: 'Basic',
          themeBuiltin: true,
          themeBase: { name: 'basic', label: 'Basic' },
          locked: h.locked[g.id] === true,
        })),
      }
    case 'zen/news':
      return { ok: true, items: [], copiesWithNewerDefault: 0 }
    case 'zen/garden/access': {
      const g = String(p.garden)
      return { ok: true, garden: g, name: h.gardens.find((x) => x.id === g)?.name ?? g, made: h.made[g] ?? null, editors: h.editors[g] ?? [] }
    }
    case 'zen/widgets':
      return { ok: true, widgets: h.widgets }
    case 'zen/gardens/deleted':
      if (!h.deletedRoute) throw new Error('unknown zen route')
      return { ok: true, gardens: h.deleted }
    case 'zen/garden/history': {
      const page = h.history[String(p.garden)]
      if (!page) throw new Error(`no history fixture for ${String(p.garden)}`)
      return page
    }
    case 'zen/widget/history': {
      const page = h.widgetHistory[String(p.widget)]
      if (!page) throw new Error(`no widget history fixture for ${String(p.widget)}`)
      return page
    }
    case 'projects/list':
      return h.agents
    case 'zen/data/spaces':
      return { ok: true, spaces: h.spaces }
    case 'zen/data/show':
      return { space: { widget: p.widget }, grants: [], delegations: [], madeBy: null, liveHash: null }
    case 'zen/get': {
      const g = String(p.garden)
      return {
        ok: true,
        garden: g,
        page: {
          widgets: [
            { id: 'talk', kind: 'conversation' },
            ...(h.pages[g] ?? []).map((w, i) => ({ id: `c${i}`, kind: 'custom', widget: w.widget, sourceAddresses: w.sourceAddresses ?? [] })),
          ],
        },
      }
    }
    case 'sources/list':
      return { sources: h.sources, serverNow: NOW, rev: 1 }
    case 'sources/status':
      return { sources: h.sources.length, active: h.sources.length, targets: 1, inFlight: 0, behindSecs: 0, parkedOrigins: [], lastGap: null, airgap: false }
    case 'sources/pending':
      return { sources: [], triggers: [] }
  }
  throw new Error(`unexpected GET ${route}`)
}

function fakePost(route: string, body: Record<string, unknown>): unknown {
  switch (route) {
    case 'zen/data/space/delete':
      h.spaces = h.spaces.filter((s) => s.widget !== body.widget)
      return { ok: true, widget: body.widget, snapshot: 's-1' }
    case 'sources/remove':
      h.sources = h.sources.filter((s) => s.name !== body.name)
      return { name: body.name, removed: true }
    case 'zen/garden/access': {
      const g = String(body.garden)
      const agent = String(body.agent)
      const list = (h.editors[g] ??= [])
      if (body.allow === true) {
        const a = h.agents.find((x) => x.id === agent)
        if (!list.some((e) => e.agent === agent)) {
          list.push({ agent, handle: String(a?.name ?? agent).toLowerCase(), name: String(a?.name ?? agent), color: '#33aa66', addedBy: 'owner', addedAt: NOW })
        }
      } else {
        h.editors[g] = list.filter((e) => e.agent !== agent)
      }
      return { ok: true, garden: g, agent, listed: body.allow === true, changed: true }
    }
    case 'zen/garden/reorder': {
      const from = h.gardens.findIndex((g) => g.id === body.garden)
      if (from < 0) throw new Error(`reorder: no Garden ${String(body.garden)}`)
      const [moved] = h.gardens.splice(from, 1)
      h.gardens.splice(Number(body.to) - 1, 0, moved)
      return { ok: true }
    }
    case 'zen/widget/sign':
      return { ok: true, widget: body.widget, hash: 'h-signed' }
    case 'zen/garden/restore':
      return { ok: true, garden: { id: body.garden, name: 'Old board' } }
  }
  throw new Error(`unexpected POST ${route}`)
}

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...mod,
    daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: Record<string, unknown>) => {
      h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
      return fakeGet(route, params ?? {})
    }),
    daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
      h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
      return fakePost(route, (body ?? {}) as Record<string, unknown>)
    }),
  }
})
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...mod,
    onZenChanged: () => () => undefined,
    onGardenDataChanged: () => () => undefined,
    subscribeToActiveState: () => () => undefined,
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenSettingsForTests } from '@/lib/zen/zen-settings'
import { __resetZenTemplatesForTests } from '@/lib/zen/zen-templates'
import { __resetZenSyncForTests } from '@/lib/zen/zen-sync'
import { __resetGardenAccessForTests, useGardenAccessSwitchStore } from '@/lib/zen/garden-access-api'
import { useToastStore } from '@/stores/toast'
import { useConnectHostStore } from '@/stores/connect-host'
import { ZenGardensSection } from './ZenGardensSection'
import { ZenWidgetData } from './ZenWidgetData'
import { GARDEN_NOT_SIGNED_TEXT } from '@/components/Zen/widgets/ZenGardenHistoryModal'
import { settingsNav } from '../Settings'
import { sectionLabel } from '../searchManifest'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { gardensLandingFor, gardensLandingSelection, requestGardensLanding, useGardensLandingStore } from '../gardens-landing'
import { buildGardenMembership, gardenPageUses, sourceNameOn } from '@/lib/zen/garden-membership'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function q(sel: string, root: ParentNode = document): HTMLElement {
  const el = root.querySelector<HTMLElement>(sel)
  if (!el) throw new Error(`missing ${sel}\n${document.body.innerHTML.slice(0, 4000)}`)
  return el
}

function posts(route: string): unknown[] {
  return h.calls.filter((c) => c.method === 'POST' && c.route === route).map((c) => c.data)
}

function gets(route: string): unknown[] {
  return h.calls.filter((c) => c.method === 'GET' && c.route === route).map((c) => c.data)
}

function setGardenAccessOn(on: boolean): void {
  useGardenAccessSwitchStore.setState({ on, loaded: true })
}

const ACTOR_ANA = { kind: 'agent', id: 'p-ana', handle: 'ana', name: 'Ana', color: '#3366ff' }

beforeEach(() => {
  h.calls = []
  h.gardenAccess = true
  h.gardens = [
    { id: 'g-1', name: 'Garden 1', template: 'k2.texting@1' },
    { id: 'g-2', name: 'Fleet Ops', template: 'k2.blank@1' },
  ]
  h.locked = { 'g-1': true }
  h.editors = {
    'g-1': [],
    'g-2': [
      { agent: 'p-ana', handle: 'ana', name: 'Ana', color: '#3366ff', addedBy: 'owner', addedAt: NOW },
      { agent: 'p-ops', handle: 'ops-bot', name: 'ops-bot', color: '#aa3366', addedBy: 'migration', addedAt: NOW },
    ],
  }
  h.made = { 'g-1': null, 'g-2': { agent: 'p-bill', handle: 'billing', name: 'Billing', at: NOW - 86400 * 10 } }
  h.deleted = [{ id: 'g-old', name: 'Old board', deletedAt: NOW - 3600, deletedBy: { kind: 'owner' } }]
  h.deletedRoute = true
  h.agents = [
    { id: 'p-zed', name: 'Zed', path: '/w/zed', color: '#111111', iconUrl: null, focusGroupId: null },
    { id: 'p-ana', name: 'Ana', path: '/w/ana', color: '#3366ff', iconUrl: null, focusGroupId: null },
    { id: 'p-bill', name: 'Billing', path: '/w/billing', color: '#22aa22', iconUrl: null, focusGroupId: null },
  ]
  h.widgets = [
    {
      name: 'fleet-rollouts',
      builtin: false,
      title: 'Rollouts',
      signedBy: { kind: 'agent', handle: 'ana', name: 'Ana', at: NOW },
      changedAt: NOW,
      placements: [{ garden: 'g-2', placement: 'r1', paused: null, signed: true }],
    },
    {
      name: 'fleet-edge',
      builtin: false,
      title: 'Edge',
      signedBy: null,
      changedAt: NOW,
      placements: [{ garden: 'g-2', placement: 'e1', paused: null, signed: false }],
    },
    { name: 'k2:diary@1', builtin: true, placements: [{ garden: 'g-2', placement: 'd', paused: null }] },
    { name: 'elsewhere', builtin: false, signedBy: null, changedAt: NOW, placements: [{ garden: 'g-1', placement: 'x' }] },
  ]
  h.history = {
    '*': { ok: true, items: [{ id: 'e-t', at: NOW, kind: 'theme', source: 'event', actor: { kind: 'owner' }, widget: null, detail: { theme: 'paper' } }], next: null, unsigned: [] },
    'g-2': {
      ok: true,
      garden: 'g-2',
      items: [
        { id: 'e-3', at: NOW, kind: 'access_added', source: 'event', actor: { kind: 'migration' }, widget: null, detail: { name: 'ops-bot' } },
        { id: 'e-2', at: NOW - 60, kind: 'changed', source: 'event', actor: ACTOR_ANA, widget: null, detail: {}, snapshot: { file: 'gardens/g-2.toml', name: 's-2' } },
      ],
      next: 'c-1',
      unsigned: [{ widget: 'fleet-edge', hash: 'h-e', gardens: ['g-2'] }],
    },
    'g-old': {
      ok: true,
      garden: 'g-old',
      items: [{ id: 'e-d', at: NOW - 3600, kind: 'deleted', source: 'event', actor: { kind: 'owner' }, widget: null, detail: {}, snapshot: { file: 'gardens/g-old.toml', name: 's-9' } }],
      next: null,
      unsigned: [{ widget: 'old-w', hash: 'h-o', gardens: ['g-old'] }],
    },
  }
  h.widgetHistory = {
    'fleet-edge': {
      ok: true,
      widget: 'fleet-edge',
      items: [{ id: 'w-1', at: NOW, kind: 'disk_edit', source: 'event', actor: { kind: 'disk' }, widget: 'fleet-edge', detail: {} }],
      next: null,
      unsigned: [{ widget: 'fleet-edge', hash: 'h-e', gardens: ['g-2'] }],
    },
  }
  h.spaces = []
  h.pages = {}
  h.sources = []
  useGardensLandingStore.setState({ landing: null })
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  __resetZenSettingsForTests()
  __resetZenTemplatesForTests()
  __resetZenSyncForTests()
  __resetGardenAccessForTests()
  useToastStore.setState({ toasts: [] })
  useConnectHostStore.setState({ activeHost: 'local' } as never)
})

afterEach(() => {
  cleanup()
})

async function mountPage(): Promise<void> {
  render(<ZenGardensSection />)
  await waitFor(() => expect(document.querySelector('[data-gardens-master-detail]')).not.toBeNull())
}

async function openGarden(id: string): Promise<HTMLElement> {
  fireEvent.click(q(`[data-gardens-side="garden"][data-gardens-side-id="${id}"]`))
  const pane = await waitFor(() => q(`[data-garden-pane="${id}"]`))
  await waitFor(() => expect(pane.querySelector('[data-garden-editors] ul')).not.toBeNull())
  return pane
}

describe('no switch (Rosson 2026-10-23)', () => {
  it('a 0.46.3+ daemon: the master-detail page, no switch row anywhere, nothing posts settings/update', async () => {
    await mountPage()
    expect(document.querySelector('[data-garden-access-switch-row]')).toBeNull()
    expect(document.querySelector('[role="switch"]')).toBeNull()
    expect(document.body.textContent).not.toContain('Garden access lists')
    fireEvent.click(q('[data-gardens-side="unplaced"]'))
    await waitFor(() => expect(document.querySelector('[data-gardens-pane="unplaced"]')).not.toBeNull())
    expect(document.querySelector('[role="switch"]')).toBeNull()
    expect(posts('settings/update')).toEqual([])
  })

  it('a daemon without the key (older than 0.46.3): today’s page, and no switch row either', async () => {
    h.gardenAccess = undefined
    render(<ZenGardensSection />)
    await waitFor(() => expect(document.querySelector('[data-zen-gardens-list]')).not.toBeNull())
    await waitFor(() => expect(gets('settings/get').length).toBeGreaterThan(0))
    expect(document.querySelector('[data-garden-access-switch-row]')).toBeNull()
    expect(document.querySelector('[data-gardens-master-detail]')).toBeNull()
    expect(document.querySelector('[data-zen-gardens-columns]')).not.toBeNull()
  })
})

describe('the sidebar (G25)', () => {
  it('Styles first, then each Garden (lock, editors, made by), + New Garden, Deleted Gardens (n), and last Not in a Garden', async () => {
    await mountPage()
    const side = q('[data-gardens-sidebar]')
    await waitFor(() => expect(side.querySelectorAll('[data-garden-side-row]').length).toBe(2))
    await waitFor(() => expect(side.querySelector('[data-gardens-deleted-toggle]')).not.toBeNull())
    const order = [...side.querySelectorAll('[data-gardens-side], [data-zen-settings-new-garden], [data-gardens-deleted-toggle]')].map(
      (el) => el.getAttribute('data-gardens-side') ?? (el.hasAttribute('data-zen-settings-new-garden') ? 'new' : 'deleted'),
    )
    expect(order).toEqual(['styles', 'garden', 'garden', 'new', 'deleted', 'unplaced'])
    expect(q('[data-gardens-side="unplaced"]').textContent).toBe('Not in a GardenData and sources no Garden uses')
    expect(q('[data-gardens-side="styles"]').getAttribute('aria-current')).toBe('page')
    const rows = [...side.querySelectorAll('[data-garden-side-row]')]
    expect(rows.map((r) => r.getAttribute('data-garden-side-row'))).toEqual(['g-1', 'g-2'])
    await waitFor(() => expect(rows[0].querySelector('[data-garden-side-locked]')).not.toBeNull())
    expect(rows[1].querySelector('[data-garden-side-locked]')).toBeNull()
    await waitFor(() => expect(q('[data-garden-side-detail]', rows[1]).textContent).toBe('2 editors · made by Billing'))
    expect(q('[data-garden-side-detail]', rows[0]).textContent).toBe('0 editors')
    await waitFor(() => expect(q('[data-gardens-deleted-toggle]').textContent).toBe('▸ Deleted Gardens (1)'))
  })

  it('Deleted Gardens hides when this daemon has no such route', async () => {
    h.deletedRoute = false
    await mountPage()
    await waitFor(() => expect(gets('zen/gardens/deleted').length).toBeGreaterThan(0))
    expect(document.querySelector('[data-gardens-deleted]')).toBeNull()
    expect(document.querySelector('[data-gardens-error]')).toBeNull()
  })

  it('Styles holds the global theme, the theme list, the zen.toml reset, the folder and the last look changes (no switch)', async () => {
    await mountPage()
    const pane = q('[data-gardens-pane="styles"]')
    expect(within(pane).queryByRole('switch')).toBeNull()
    await waitFor(() => expect(pane.querySelectorAll('[data-gardens-theme-list] li').length).toBe(2))
    expect(q('[data-gardens-reset-zen]', pane).textContent).toBe('Reset')
    expect(q('[data-zen-gardens-path]', pane).textContent).toBe('/Users/r/.k2/zen')
    await waitFor(() => expect(q('[data-gardens-look-line="e-t"]', pane).textContent).toContain('You changed the theme to paper'))
    expect(gets('zen/garden/history')).toContainEqual({ garden: '*', limit: 20 })
  })
})

describe('the sidebar reorders by dragging the row, no handle (Rosson 2026-10-23)', () => {
  const ROW_H = 40

  /** jsdom has no layout: give Garden row i the band [i*40, i*40+40). */
  function layOutRows(): void {
    const rows = [...document.querySelectorAll<HTMLElement>('[data-garden-side-row]')]
    rows.forEach((r, i) => {
      r.getBoundingClientRect = () =>
        ({ top: i * ROW_H, bottom: i * ROW_H + ROW_H, height: ROW_H, left: 0, right: 240, width: 240, x: 0, y: i * ROW_H, toJSON: () => ({}) }) as DOMRect
    })
  }

  function row(id: string): HTMLElement {
    return q(`[data-gardens-side="garden"][data-gardens-side-id="${id}"]`)
  }

  function sideOrder(): Array<string | null> {
    return [...document.querySelectorAll('[data-garden-side-row]')].map((r) => r.getAttribute('data-garden-side-row'))
  }

  async function mountWithRows(): Promise<void> {
    await mountPage()
    await waitFor(() => expect(document.querySelectorAll('[data-garden-side-row]').length).toBe(2))
    layOutRows()
  }

  it('renders no handle: no grip, no drag-handle role, no "Reorder" button; the row itself carries the hint', async () => {
    await mountWithRows()
    const side = q('[data-gardens-sidebar]')
    expect(side.querySelector('[data-garden-side-grip]')).toBeNull()
    expect(side.querySelector('[aria-roledescription="drag handle"]')).toBeNull()
    expect(within(side).queryAllByRole('button', { name: /^Reorder / })).toEqual([])
    for (const id of ['g-1', 'g-2']) {
      const r = q(`[data-garden-side-row="${id}"]`)
      expect(r.querySelectorAll('button').length).toBe(1)
      expect(row(id).className).toContain('cursor-pointer')
    }
  })

  it('a plain click (or a wobble under 4 px) selects the Garden and does not reorder', async () => {
    await mountWithRows()
    const r = row('g-2')
    fireEvent.mouseDown(r, { button: 0, clientX: 10, clientY: 60 })
    fireEvent.mouseMove(document, { clientX: 12, clientY: 62 })
    expect(document.querySelector('[data-garden-side-drop]')).toBeNull()
    fireEvent.mouseUp(document, { clientX: 12, clientY: 62 })
    fireEvent.click(r)
    await waitFor(() => expect(document.querySelector('[data-garden-pane="g-2"]')).not.toBeNull())
    expect(row('g-2').getAttribute('aria-current')).toBe('page')
    expect(posts('zen/garden/reorder')).toEqual([])
    expect(sideOrder()).toEqual(['g-1', 'g-2'])
  })

  it('dragging a row past the next one shows the drop line, posts the same zen/garden/reorder and does not select', async () => {
    await mountWithRows()
    const r = row('g-1')
    fireEvent.mouseDown(r, { button: 0, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 75 })
    await waitFor(() => expect(q('[data-garden-side-drop="bottom"]', q('[data-garden-side-row="g-2"]'))).not.toBeNull())
    expect(q('[data-garden-side-row="g-1"]').className).toContain('opacity-30')
    expect(row('g-1').className).toContain('cursor-grabbing')
    expect(document.body.style.cursor).toBe('grabbing')
    fireEvent.mouseUp(document, { clientX: 10, clientY: 75 })
    fireEvent.click(r)
    expect(posts('zen/garden/reorder')).toEqual([{ garden: 'g-1', to: 2 }])
    await waitFor(() => expect(sideOrder()).toEqual(['g-2', 'g-1']))
    expect(document.querySelector('[data-garden-side-drop]')).toBeNull()
    expect(document.body.style.cursor).toBe('')
    expect(q('[data-gardens-side="styles"]').getAttribute('aria-current')).toBe('page')
    expect(document.querySelector('[data-garden-pane="g-1"]')).toBeNull()
    expect(useToastStore.getState().toasts).toEqual([])
  })

  it('the fixed entries (Styles, + New Garden, Deleted Gardens, Not in a Garden) are not draggable', async () => {
    await mountWithRows()
    await waitFor(() => expect(document.querySelector('[data-gardens-deleted-toggle]')).not.toBeNull())
    const side = q('[data-gardens-sidebar]')
    expect([...side.querySelectorAll('[data-garden-draggable]')].map((el) => el.getAttribute('data-gardens-side-id'))).toEqual(['g-1', 'g-2'])
    const fixed = [
      q('[data-gardens-side="styles"]'),
      q('[data-zen-settings-new-garden]'),
      q('[data-gardens-deleted-toggle]'),
      q('[data-gardens-side="unplaced"]'),
    ]
    for (const el of fixed) {
      expect(el.hasAttribute('data-garden-draggable')).toBe(false)
      expect(el.closest('[data-garden-side-row]')).toBeNull()
      fireEvent.mouseDown(el, { button: 0, clientX: 10, clientY: 0 })
      fireEvent.mouseMove(document, { clientX: 10, clientY: 75 })
      expect(document.querySelector('[data-garden-side-drop]')).toBeNull()
      fireEvent.mouseUp(document, { clientX: 10, clientY: 75 })
    }
    expect(posts('zen/garden/reorder')).toEqual([])
    expect(sideOrder()).toEqual(['g-1', 'g-2'])
  })

  it('Alt+↑ / Alt+↓ on the focused row moves it (described for screen readers); plain arrows and the ends do nothing', async () => {
    await mountWithRows()
    const r2 = row('g-2')
    expect(r2.getAttribute('aria-keyshortcuts')).toBe('Alt+ArrowUp Alt+ArrowDown')
    const help = q(`#${r2.getAttribute('aria-describedby')}`)
    expect(help.textContent).toBe('Drag a Garden to reorder it, or press Alt+Up Arrow or Alt+Down Arrow to move it.')
    expect(help.className).toContain('sr-only')

    r2.focus()
    fireEvent.keyDown(r2, { key: 'ArrowUp' })
    fireEvent.keyDown(r2, { key: 'ArrowDown', altKey: true })
    fireEvent.keyDown(row('g-1'), { key: 'ArrowUp', altKey: true })
    expect(posts('zen/garden/reorder')).toEqual([])

    fireEvent.keyDown(r2, { key: 'ArrowUp', altKey: true })
    expect(posts('zen/garden/reorder')).toEqual([{ garden: 'g-2', to: 1 }])
    await waitFor(() => expect(sideOrder()).toEqual(['g-2', 'g-1']))
    await waitFor(() => expect(document.activeElement).toBe(row('g-2')))

    await waitFor(() => expect(h.gardens.map((g) => g.id)).toEqual(['g-2', 'g-1']))
    // Let the first move's promise settle (one move in flight at a time).
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    fireEvent.keyDown(row('g-2'), { key: 'ArrowDown', altKey: true })
    expect(posts('zen/garden/reorder')).toEqual([
      { garden: 'g-2', to: 1 },
      { garden: 'g-2', to: 2 },
    ])
    await waitFor(() => expect(sideOrder()).toEqual(['g-1', 'g-2']))
    expect(q('[data-gardens-side="styles"]').getAttribute('aria-current')).toBe('page')
    expect(useToastStore.getState().toasts).toEqual([])
  })
})

describe('the Garden pane (G26)', () => {
  it('Made by, editors (auto-add line), Sees, Lock with its caption, design updates, widgets with their signer; nothing "not signed"', async () => {
    await mountPage()
    const pane = await openGarden('g-2')
    expect(q('[data-garden-pane-title]', pane).textContent).toBe('Fleet Ops')
    await waitFor(() => expect(q('[data-garden-pane-made]', pane).textContent).toMatch(/^Made by Billing · \d+ \w{3}$/))
    const editors = [...pane.querySelectorAll('[data-garden-editor]')]
    expect(editors.map((e) => e.getAttribute('data-garden-editor'))).toEqual(['p-ana', 'p-ops'])
    expect(q('[data-garden-editor-added]', editors[0]).textContent).toMatch(/^added by you · /)
    expect(q('[data-garden-editor-added]', editors[1]).textContent).toMatch(/^added automatically at upgrade · /)
    expect(q('[data-garden-sees]', pane).textContent).toBe('SeesEverything you can see')
    await waitFor(() => expect(pane.querySelector('[data-zen-garden-lock="g-2"]')).not.toBeNull())
    expect(q('[data-zen-garden-lock="g-2"]', pane).textContent).toBe('Lock gardenAgents can’t change this Garden. You still can, and K2’s updates to its libraries still apply.')
    expect(q('[data-zen-garden-sync="g-2"]', pane).textContent).toContain('Get K2’s design updates')
    await waitFor(() => expect(pane.querySelectorAll('[data-garden-widget]').length).toBe(2))
    const widgets = [...pane.querySelectorAll('[data-garden-widget]')]
    expect(widgets.map((w) => w.getAttribute('data-garden-widget'))).toEqual(['fleet-rollouts', 'fleet-edge'])
    expect(q('[data-garden-widget-signer]', widgets[0]).textContent).toMatch(/^last signed by Ana · /)
    expect(q('[data-garden-widget-signer]', widgets[1]).textContent).toMatch(/^last changed /)
    // "Not signed" and Sign live only in the modals (G26a).
    expect(document.body.textContent).not.toContain('not signed')
    expect(document.body.textContent).not.toContain(GARDEN_NOT_SIGNED_TEXT)
    expect(document.querySelector('[data-garden-sign]')).toBeNull()
  })

  it('Remove posts allow:false, then the 8 s Undo posts allow:true', async () => {
    await mountPage()
    const pane = await openGarden('g-2')
    await act(async () => fireEvent.click(q('[data-garden-editor-remove="p-ana"]', pane)))
    expect(posts('zen/garden/access')).toEqual([{ garden: 'g-2', agent: 'p-ana', allow: false }])
    await waitFor(() => expect(pane.querySelector('[data-garden-editor="p-ana"]')).toBeNull())
    const toast = useToastStore.getState().toasts.at(-1)
    if (!toast) throw new Error('no toast after Remove')
    expect(toast.message).toBe('Ana can no longer edit Fleet Ops')
    expect(toast.duration).toBe(8000)
    expect(toast.action?.label).toBe('Undo')
    await act(async () => toast.action?.onClick())
    expect(posts('zen/garden/access')).toEqual([
      { garden: 'g-2', agent: 'p-ana', allow: false },
      { garden: 'g-2', agent: 'p-ana', allow: true },
    ])
    await waitFor(() => expect(pane.querySelector('[data-garden-editor="p-ana"]')).not.toBeNull())
  })

  it('+ Add agent lists this computer’s agents A–Z (listed ones checked) and posts allow:true', async () => {
    await mountPage()
    const pane = await openGarden('g-2')
    await act(async () => fireEvent.click(q('[data-garden-add-agent]', pane)))
    const picker = await waitFor(() => q('[data-garden-add-agent-picker]'))
    await waitFor(() => expect(within(picker).getAllByRole('option').length).toBe(3))
    const options = within(picker).getAllByRole('option')
    expect(options.map((o) => o.textContent?.trim().replace(/\s+/g, ' '))).toEqual([
      expect.stringContaining('Ana'),
      expect.stringContaining('Billing'),
      expect.stringContaining('Zed'),
    ])
    expect(gets('projects/list').length).toBe(1)
    expect(h.calls.filter((c) => c.route === 'projects/list').map((c) => c.hostKey)).toEqual(['local'])
    // Ana is already listed: checked, not pickable.
    await act(async () => fireEvent.click(options[0]))
    expect(posts('zen/garden/access')).toEqual([])
    await act(async () => fireEvent.click(within(picker).getByText('Billing')))
    expect(posts('zen/garden/access')).toEqual([{ garden: 'g-2', agent: 'p-bill', allow: true }])
    await waitFor(() => expect(pane.querySelector('[data-garden-editor="p-bill"]')).not.toBeNull())
    expect(useToastStore.getState().toasts.at(-1)?.message).toBe('Billing can edit Fleet Ops')
  })

  it('an empty list says how agents get on it', async () => {
    await mountPage()
    const pane = await openGarden('g-1')
    expect(q('[data-garden-editors-empty]', pane).textContent).toBe(
      'No agents yet. An agent that asks gets a card on its Thread with you.',
    )
  })
})

describe('the history modals (G26a)', () => {
  it('History… opens the Garden modal: title, filters, lines, Load more, and the unsigned widget with Sign', async () => {
    await mountPage()
    const pane = await openGarden('g-2')
    await act(async () => fireEvent.click(q('[data-garden-pane-history]', pane)))
    const modal = await waitFor(() => screen.getByTestId('zen-garden-history-modal'))
    expect(q('[data-garden-history-title]', modal).textContent).toBe('Fleet Ops history')
    expect([...modal.querySelectorAll('[data-garden-history-filter]')].map((b) => b.textContent)).toEqual(['All', 'Changes', 'Access', 'Asks'])
    await waitFor(() => expect(modal.querySelectorAll('[data-garden-history-line]').length).toBe(2))
    expect(q('[data-garden-history-line="e-3"] [data-garden-history-text]', modal).textContent).toBe('ops-bot was added automatically at upgrade')
    expect(q('[data-garden-history-line="e-2"] [data-garden-history-text]', modal).textContent).toBe('Ana changed the layout')
    expect(q('[data-garden-history-line="e-2"] [data-garden-history-restore]', modal).textContent).toBe('Restore')
    expect(q('[data-garden-not-signed="fleet-edge"]', modal).textContent).toContain(GARDEN_NOT_SIGNED_TEXT)
    expect(q('[data-garden-not-signed="fleet-edge"]', modal).textContent).toContain('fleet-edge')
    expect(gets('zen/garden/history')).toContainEqual({ garden: 'g-2', limit: 50 })
    await act(async () => fireEvent.click(q('[data-garden-sign="fleet-edge"]', modal)))
    expect(posts('zen/widget/sign')).toEqual([{ widget: 'fleet-edge', garden: 'g-2' }])
    // Load more asks for the next page after the cursor.
    await waitFor(() => expect(modal.querySelector('[data-garden-history-more]')).not.toBeNull())
    await act(async () => fireEvent.click(q('[data-garden-history-more]', modal)))
    expect(gets('zen/garden/history')).toContainEqual({ garden: 'g-2', limit: 50, before: 'c-1' })
  })

  it('a widget’s History… opens its modal, which alone says "not signed" with Sign', async () => {
    await mountPage()
    const pane = await openGarden('g-2')
    await waitFor(() => expect(pane.querySelector('[data-garden-widget-history="fleet-edge"]')).not.toBeNull())
    await act(async () => fireEvent.click(q('[data-garden-widget-history="fleet-edge"]', pane)))
    const modal = await waitFor(() => screen.getByTestId('zen-garden-history-modal'))
    expect(q('[data-garden-history-title]', modal).textContent).toBe('fleet-edge history')
    await waitFor(() => expect(modal.querySelector('[data-garden-not-signed="fleet-edge"]')).not.toBeNull())
    expect(q('[data-garden-not-signed="fleet-edge"]', modal).textContent).toBe(`${GARDEN_NOT_SIGNED_TEXT}Sign`)
    expect(q('[data-garden-history-line="w-1"] [data-garden-history-text]', modal).textContent).toBe('Outside K2 changed fleet-edge')
    expect(gets('zen/widget/history')).toEqual([{ widget: 'fleet-edge', limit: 50 }])
    await act(async () => fireEvent.click(q('[data-garden-sign="fleet-edge"]', modal)))
    expect(posts('zen/widget/sign')).toEqual([{ widget: 'fleet-edge', garden: 'g-2' }])
  })
})

describe('Deleted Gardens (G25)', () => {
  it('Restore posts zen/garden/restore; its History is read-only (no Sign, no Restore)', async () => {
    await mountPage()
    await waitFor(() => expect(document.querySelector('[data-gardens-deleted-toggle]')).not.toBeNull())
    fireEvent.click(q('[data-gardens-deleted-toggle]'))
    fireEvent.click(q('[data-gardens-side="deleted"][data-gardens-side-id="g-old"]'))
    const pane = q('[data-garden-deleted-pane="g-old"]')
    expect(pane.textContent).toContain('Deleted by you')
    await act(async () => fireEvent.click(q('[data-garden-deleted-history]', pane)))
    const modal = await waitFor(() => screen.getByTestId('zen-garden-history-modal'))
    await waitFor(() => expect(modal.querySelectorAll('[data-garden-history-line]').length).toBe(1))
    expect(q('[data-garden-not-signed="old-w"]', modal).textContent).toContain(GARDEN_NOT_SIGNED_TEXT)
    expect(modal.querySelector('[data-garden-sign]')).toBeNull()
    expect(modal.querySelector('[data-garden-history-restore]')).toBeNull()
    expect(modal.querySelector('[data-garden-history-view]')).not.toBeNull()
    fireEvent.click(within(modal).getByRole('button', { name: 'Close' }))
    await act(async () => fireEvent.click(q('[data-garden-restore]', pane)))
    expect(posts('zen/garden/restore')).toEqual([{ garden: 'g-old' }])
  })
})

describe('Widget data (G28)', () => {
  const SPACE = {
    widget: 'fleet-edge',
    bytes: 1200,
    keys: 3,
    rev: 1,
    grants: 0,
    admins: [],
    orphanedAt: null,
    madeBy: { kind: 'agent', handle: 'ana' },
  }

  it('on: "Widget data", the last signer, no It’s mine or Let … use, grants under Advanced', async () => {
    h.spaces = [SPACE]
    h.widgets = [{ name: 'fleet-edge', builtin: false, title: null }]
    setGardenAccessOn(true)
    render(<ZenWidgetData />)
    await waitFor(() => expect(document.querySelector('[data-garden-data-line="fleet-edge"]')).not.toBeNull())
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Widget data')
    const line = q('[data-garden-data-line="fleet-edge"]')
    expect(q('[data-garden-data-summary]', line).textContent).toContain('Last signed by ana')
    expect(line.textContent).not.toContain('It\'s mine')
    expect(line.textContent).not.toContain('use this data')
    await act(async () => fireEvent.click(within(line).getByRole('button', { name: 'Advanced' })))
    const adv = await waitFor(() => q('[data-garden-data-advanced="fleet-edge"]'))
    expect(adv.textContent).toContain('Advanced · data grants')
    expect(adv.querySelector('[data-garden-data-grants="fleet-edge"]')).not.toBeNull()
  })

  it('off: Local Data as 0.46.3 (Made by, It’s mine, Grants)', async () => {
    h.spaces = [SPACE]
    h.widgets = [{ name: 'fleet-edge', builtin: false, title: null }]
    setGardenAccessOn(false)
    render(<ZenWidgetData />)
    await waitFor(() => expect(document.querySelector('[data-garden-data-line="fleet-edge"]')).not.toBeNull())
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Local Data')
    const line = q('[data-garden-data-line="fleet-edge"]')
    expect(q('[data-garden-data-summary]', line).textContent).toContain('Made by ana')
    expect(within(line).getByRole('button', { name: 'It\'s mine' })).toBeTruthy()
    expect(within(line).getByRole('button', { name: 'Let ana use this data' })).toBeTruthy()
    expect(within(line).getByRole('button', { name: 'Grants' })).toBeTruthy()
  })
})

describe('the nav (Rosson 2026-10-23: Gardens is one page)', () => {
  const top = (zenGardens: boolean): string[] =>
    settingsNav({ zenGardens }).flatMap((b) => (b.kind === 'item' ? [b.id] : []))
  const leaves = (zenGardens: boolean): string[] =>
    settingsNav({ zenGardens }).flatMap((b) => (b.kind === 'group' ? b.items.flatMap((i) => (i.id ? [i.id] : [])) : []))

  it('with Gardens: one "Gardens" item between Tickets and Context Catalog, no Gardens group, no data items', () => {
    const items = top(true)
    const at = items.indexOf('zen-gardens')
    expect(items[at - 1]).toBe('tickets')
    expect(items[at + 1]).toBe('context-catalog')
    const nav = settingsNav({ zenGardens: true })
    expect(nav.find((b) => b.kind === 'item' && b.id === 'zen-gardens')).toEqual({ kind: 'item', id: 'zen-gardens', label: 'Gardens' })
    expect(nav.some((b) => b.kind === 'group' && b.title === 'Gardens')).toBe(false)
    const all = [...items, ...leaves(true)]
    expect(all).not.toContain('zen-widget-data')
    expect(all).not.toContain('data-sources')
    expect(all.filter((id) => id === 'zen-gardens')).toHaveLength(1)
  })

  it('without Gardens (web, Windows): no Gardens item, External Data stays in Network', () => {
    expect(top(false)).not.toContain('zen-gardens')
    const network = settingsNav({ zenGardens: false }).find((b) => b.kind === 'group' && b.title === 'Network')
    if (!network || network.kind !== 'group') throw new Error('no Network group')
    expect(network.items.map((i) => i.id).at(-1)).toBe('data-sources')
  })
})

describe('search landing', () => {
  it('Local Data and External Data results land on Gardens (with their Garden when they know it); without Gardens they stay', () => {
    expect(gardensLandingFor('data-sources', undefined, true)).toEqual({
      section: 'zen-gardens',
      landing: { garden: null, part: 'sources' },
    })
    expect(gardensLandingFor('zen-widget-data', 'g-2', true)).toEqual({
      section: 'zen-gardens',
      landing: { garden: 'g-2', part: 'data' },
    })
    expect(gardensLandingFor('zen-gardens', 'g-1', true)).toEqual({ section: 'zen-gardens', landing: { garden: 'g-1', part: null } })
    expect(gardensLandingFor('zen-gardens', undefined, true)).toEqual({ section: 'zen-gardens', landing: null })
    expect(gardensLandingFor('data-sources', undefined, false)).toEqual({ section: 'data-sources', landing: null })
    expect(gardensLandingFor('tickets', undefined, true)).toEqual({ section: 'tickets', landing: null })
    expect(sectionLabel('data-sources', { gardensPage: true })).toBe('Gardens')
    expect(sectionLabel('zen-widget-data', { gardensPage: true })).toBe('Gardens')
    expect(sectionLabel('data-sources')).toBe('External Data')
    expect(sectionLabel('zen-widget-data')).toBe('Local Data')
  })

  it('a landing opens its Garden when it exists, else Not in a Garden; a plain Gardens landing changes nothing', () => {
    const ids = ['g-1', 'g-2']
    expect(gardensLandingSelection({ garden: 'g-2', part: 'sources' }, ids)).toEqual({ kind: 'garden', id: 'g-2' })
    expect(gardensLandingSelection({ garden: 'g-gone', part: 'data' }, ids)).toEqual({ kind: 'unplaced' })
    expect(gardensLandingSelection({ garden: null, part: 'sources' }, ids)).toEqual({ kind: 'unplaced' })
    expect(gardensLandingSelection({ garden: null, part: null }, ids)).toBeNull()
  })

  it('the page takes a pending landing once: a Garden’s, then Not in a Garden', async () => {
    requestGardensLanding({ garden: 'g-2', part: 'data' })
    await mountPage()
    await waitFor(() => expect(document.querySelector('[data-garden-pane="g-2"]')).not.toBeNull())
    expect(useGardensLandingStore.getState().landing).toBeNull()
    await act(async () => requestGardensLanding({ garden: null, part: 'sources' }))
    await waitFor(() => expect(document.querySelector('[data-gardens-pane="unplaced"]')).not.toBeNull())
    expect(q('[data-gardens-side="unplaced"]').getAttribute('aria-current')).toBe('page')
  })
})

function sourceRow(name: string, label: string): Record<string, unknown> {
  return {
    name,
    label,
    status: 'active',
    madeBy: 'agent:p-ana',
    secret: null,
    pinned: false,
    keep: 'values',
    viewers: 'member',
    agentAccess: 'values',
    everySecs: 60,
    targets: 1,
    triggers: 0,
    view: {
      name,
      label,
      kind: 'http-json',
      status: 'active',
      health: 'ok',
      phase: 'live',
      display: 'ok',
      reason: null,
      reasonText: null,
      httpStatus: null,
      soft: { count: 0, of: 3 },
      serverNow: NOW,
      updatedAt: NOW - 30,
      checkedAt: NOW - 30,
      nextAt: NOW + 30,
      hardSince: null,
      recoveredAt: null,
      gap: null,
      muted: null,
      simulated: null,
      pinned: false,
      keep: 'values',
      masked: false,
      link: null,
      fields: {},
      items: [],
      counts: { ok: 1, warn: 0, crit: 0, unknown: 0 },
      rev: 1,
    },
  }
}

function space(widget: string): Record<string, unknown> {
  return { widget, bytes: 900, keys: 2, rev: 1, grants: 0, admins: [], orphanedAt: null, madeBy: { kind: 'owner' } }
}

describe('a Garden’s Local Data and External Data (Rosson 2026-10-23)', () => {
  beforeEach(() => {
    // `board` is placed in both Gardens; `rollouts` only in Fleet Ops;
    // `drafty` nowhere. Source `fleet` is read by both Gardens, `deploys`
    // by Fleet Ops, `money` by nobody, `far` lives on another server.
    h.pages = {
      'g-1': [{ widget: 'board', sourceAddresses: ['fleet'] }],
      'g-2': [
        { widget: 'board', sourceAddresses: ['fleet'] },
        { widget: 'rollouts', sourceAddresses: ['deploys', 'far::akzm.k2.dev'] },
      ],
    }
    h.widgets = [
      { name: 'board', builtin: false, title: 'Board', placements: [{ garden: 'g-1', placement: 'c0' }, { garden: 'g-2', placement: 'c0' }] },
      { name: 'rollouts', builtin: false, title: 'Rollouts', placements: [{ garden: 'g-2', placement: 'c1' }] },
      { name: 'drafty', builtin: false, title: null, placements: [] },
    ]
    h.spaces = [space('board'), space('rollouts'), space('drafty'), { ...space('gone-w'), orphanedAt: NOW - 60 }]
    h.sources = [sourceRow('fleet', 'Fleet boxes'), sourceRow('deploys', 'Deploys'), sourceRow('money', 'Money')]
  })

  const dataLines = (root: ParentNode): string[] =>
    [...root.querySelectorAll('[data-garden-data-line]')].map((el) => el.getAttribute('data-garden-data-line') ?? '')
  const sourceNames = (root: ParentNode): string[] =>
    [...root.querySelectorAll('[data-testid="sources-row"]')].map((el) => el.getAttribute('data-source') ?? '')

  it('each Garden shows its widgets’ data and its sources; a widget or source in two Gardens shows in both', async () => {
    await mountPage()
    const ops = await openGarden('g-2')
    const opsLocal = await waitFor(() => q('[data-garden-local-data="g-2"]', ops))
    await waitFor(() => expect(dataLines(opsLocal)).toEqual(['board', 'rollouts']))
    expect(q('h3', opsLocal).textContent).toBe('Local Data')
    expect(opsLocal.querySelector('[data-garden-data-orphans]')).toBeNull()
    const opsExt = q('[data-garden-external-data="g-2"]', ops)
    await waitFor(() => expect(sourceNames(opsExt)).toEqual(['fleet', 'deploys']))
    expect(q('h3', opsExt).textContent).toBe('External Data')

    const g1 = await openGarden('g-1')
    const g1Local = await waitFor(() => q('[data-garden-local-data="g-1"]', g1))
    await waitFor(() => expect(dataLines(g1Local)).toEqual(['board']))
    await waitFor(() => expect(sourceNames(q('[data-garden-external-data="g-1"]', g1))).toEqual(['fleet']))
    // One zen/get per Garden, from this computer's daemon.
    expect(h.calls.filter((c) => c.route === 'zen/get').every((c) => c.hostKey === 'local')).toBe(true)
    expect(gets('zen/get')).toEqual(expect.arrayContaining([{ garden: 'g-1' }, { garden: 'g-2' }]))
  })

  it('Not in a Garden: the data of widgets no Garden places, data with no widget, and sources nobody reads', async () => {
    await mountPage()
    fireEvent.click(q('[data-gardens-side="unplaced"]'))
    const pane = await waitFor(() => q('[data-gardens-pane="unplaced"]'))
    expect(q('h2', pane).textContent).toBe('Not in a Garden')
    await waitFor(() => expect(dataLines(pane)).toEqual(['drafty']))
    await waitFor(() => expect(pane.querySelector('[data-garden-data-orphan="gone-w"]')).not.toBeNull())
    await waitFor(() => expect(sourceNames(pane)).toEqual(['money']))
  })

  it('the old actions work in a pane: Delete data and Remove source post what the old pages posted', async () => {
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) } as never)
    await mountPage()
    const ops = await openGarden('g-2')
    const local = await waitFor(() => q('[data-garden-local-data="g-2"]', ops))
    const line = await waitFor(() => q('[data-garden-data-line="rollouts"]', local))
    await act(async () => fireEvent.click(within(line).getByRole('button', { name: 'Delete' })))
    expect(posts('zen/data/space/delete')).toEqual([{ widget: 'rollouts', confirm: 'rollouts' }])
    const ext = q('[data-garden-external-data="g-2"]', ops)
    const row = await waitFor(() => q('[data-source="deploys"]', ext))
    await act(async () => fireEvent.click(within(row).getByTestId('sources-row-menu')))
    await act(async () => fireEvent.click(within(row).getByTestId('sources-menu-remove')))
    await waitFor(() => expect(posts('sources/remove')).toEqual([{ name: 'deploys' }]))
  })

  it('membership is pure: pages → per-Garden widgets and this server’s source names', () => {
    expect(gardenPageUses({ page: { widgets: [{ kind: 'custom', widget: 'a', sourceAddresses: ['x', 'y::akzm.k2.dev'] }, { kind: 'clock' }] } })).toEqual({
      widgets: ['a'],
      sources: ['x', 'y::akzm.k2.dev'],
    })
    expect(gardenPageUses({ widgets: [{ kind: 'custom', widget: 'b' }] })).toEqual({ widgets: ['b'], sources: [] })
    expect(sourceNameOn('fleet', 'local')).toBe('fleet')
    expect(sourceNameOn('fleet', 'akzm.k2.dev')).toBeNull()
    expect(sourceNameOn('fleet::akzm.k2.dev', 'akzm.k2.dev')).toBe('fleet')
    expect(sourceNameOn('fleet::AKZM.k2.dev:443', 'akzm.k2.dev')).toBe('fleet')
    expect(sourceNameOn('fleet::other.k2.dev', 'akzm.k2.dev')).toBeNull()
    expect(sourceNameOn('fleet::not a host', 'local')).toBeNull()
    const m = buildGardenMembership(
      new Map([
        ['g-1', { widgets: ['board'], sources: ['fleet'] }],
        ['g-2', { widgets: ['board', 'rollouts'], sources: ['fleet', 'far::akzm.k2.dev'] }],
      ]),
      'local',
    )
    expect([...(m.widgets.get('g-1') ?? [])]).toEqual(['board'])
    expect([...m.placedWidgets].sort()).toEqual(['board', 'rollouts'])
    expect([...m.usedSources]).toEqual(['fleet'])
    expect([...(m.sources.get('g-2') ?? [])]).toEqual(['fleet'])
  })
})
