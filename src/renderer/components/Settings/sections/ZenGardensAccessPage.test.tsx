// @vitest-environment jsdom
//
// prd-garden-access-v1 §6 (A5): Settings → Gardens with Garden access.
// The real section, the real page, the real stores; only the edges are
// faked: the daemon (`daemon-cli`, a small in-memory Gardens + access
// daemon), the app socket, Tauri and the opener plugin.
//
// Asserted (fail loudly):
//   - switch absent (an older daemon): today's page, no switch row;
//   - switch off: today's page plus the switch row; the row posts
//     `settings/update {gardenAccess: true}` to the LOCAL daemon;
//   - switch on: the master-detail page; the sidebar is Styles first, then
//     each Garden (lock glyph, "N editors · made by X"), + New Garden, then
//     Deleted Gardens (n);
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
  }
  throw new Error(`unexpected GET ${route}`)
}

function fakePost(route: string, body: Record<string, unknown>): unknown {
  switch (route) {
    case 'settings/update':
      h.gardenAccess = body.gardenAccess === true
      return { gardenAccess: h.gardenAccess }
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

describe('the switch (G0a, G27)', () => {
  it('a daemon without the switch: today’s page, no switch row, no master-detail', async () => {
    h.gardenAccess = undefined
    render(<ZenGardensSection />)
    await waitFor(() => expect(document.querySelector('[data-zen-gardens-list]')).not.toBeNull())
    await waitFor(() => expect(gets('settings/get').length).toBeGreaterThan(0))
    expect(document.querySelector('[data-garden-access-switch-row]')).toBeNull()
    expect(document.querySelector('[data-gardens-master-detail]')).toBeNull()
    expect(document.querySelector('[data-zen-gardens-columns]')).not.toBeNull()
  })

  it('off: today’s page plus the switch row; flipping it posts gardenAccess:true to the local daemon and shows the new page', async () => {
    h.gardenAccess = false
    render(<ZenGardensSection />)
    const row = await waitFor(() => q('[data-garden-access-switch-row]'))
    expect(document.querySelector('[data-zen-gardens-list]')).not.toBeNull()
    expect(document.querySelector('[data-gardens-master-detail]')).toBeNull()
    // Today's per-row controls are still there, with today's words.
    await waitFor(() => expect(document.querySelector('[data-zen-garden-sync="g-1"]')).not.toBeNull())
    expect(q('[data-zen-garden-sync="g-1"]').textContent).toContain('Keep in sync with future template updates')
    const sw = within(row).getByRole('switch', { name: 'Garden access lists' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    await act(async () => fireEvent.click(sw))
    expect(posts('settings/update')).toEqual([{ gardenAccess: true }])
    expect(h.calls.filter((c) => c.route === 'settings/update').map((c) => c.hostKey)).toEqual(['local'])
    await waitFor(() => expect(document.querySelector('[data-gardens-master-detail]')).not.toBeNull())
  })
})

describe('the sidebar (G25)', () => {
  it('Styles first, then each Garden (lock, editors, made by), + New Garden, then Deleted Gardens (n)', async () => {
    await mountPage()
    const side = q('[data-gardens-sidebar]')
    await waitFor(() => expect(side.querySelectorAll('[data-garden-side-row]').length).toBe(2))
    const order = [...side.querySelectorAll('[data-gardens-side], [data-zen-settings-new-garden], [data-gardens-deleted-toggle]')].map(
      (el) => el.getAttribute('data-gardens-side') ?? (el.hasAttribute('data-zen-settings-new-garden') ? 'new' : 'deleted'),
    )
    expect(order).toEqual(['styles', 'garden', 'garden', 'new', 'deleted'])
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

  it('Styles holds the switch, the global theme, the theme list, the zen.toml reset, the folder and the last look changes', async () => {
    await mountPage()
    const pane = q('[data-gardens-pane="styles"]')
    expect(within(pane).getByRole('switch', { name: 'Garden access lists' }).getAttribute('aria-checked')).toBe('true')
    await waitFor(() => expect(pane.querySelectorAll('[data-gardens-theme-list] li').length).toBe(2))
    expect(q('[data-gardens-reset-zen]', pane).textContent).toBe('Reset')
    expect(q('[data-zen-gardens-path]', pane).textContent).toBe('/Users/r/.k2/zen')
    await waitFor(() => expect(q('[data-gardens-look-line="e-t"]', pane).textContent).toContain('You changed the theme to paper'))
    expect(gets('zen/garden/history')).toContainEqual({ garden: '*', limit: 20 })
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
    expect(q('[data-zen-garden-lock="g-2"]', pane).textContent).toBe('Lock gardenAgents can’t change this Garden. You still can.')
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

describe('the nav name (G28)', () => {
  it('Garden access on: "Widget data"; off: Local Data, as 0.46.3', () => {
    const leaf = (gardenAccess: boolean): string | undefined =>
      settingsNav({ zenGardens: true, gardenAccess })
        .flatMap((b) => (b.kind === 'group' ? b.items : []))
        .find((it) => it.id === 'zen-widget-data')?.label
    expect(leaf(true)).toBe('Widget data')
    expect(leaf(false)).toBe('Local Data')
    expect(sectionLabel('zen-widget-data', { gardenAccess: true })).toBe('Widget data')
    expect(sectionLabel('zen-widget-data')).toBe('Local Data')
  })
})
