// @vitest-environment jsdom
//
// prd-garden-access-v1 §8 / G0a: the switch-off golden for Settings → Gardens
// (`tests/golden/garden_access_off/renderer/settings_gardens_off.html`).
//
// With `gardenAccess` off the page is exactly 0.46.3's, plus ONE named
// exception (G0a item 1): the switch row ("Garden access lists"), drawn only
// by a daemon that has the switch. The golden was captured from
// integ/0.46.3-r9 (no Garden access code). On a tree that has the switch,
// this test requires the row (switched off), lifts it out, and compares the rest
// of the page byte for byte. On a tree without it, there is no row to lift.
//
// The fake daemon and edges are 0.46.3's ZenGardensSection.test.tsx harness;
// `settings/get` answers `gardenAccess: false` (the switch off).
//
// Modes: plain run compares and fails loudly on any difference or a missing
// golden. GARDEN_ACCESS_GOLDEN_BLESS=1 writes the golden and prints it as
// base64 between markers (capture only ever from a base WITHOUT Garden access).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type FakeGarden = { id: string; name: string; template: string; theme: string | null }

const h = vi.hoisted(() => ({
  gardens: [] as FakeGarden[],
  global: 'default',
  themes: ['default', 'paper', 'midnight'],
  sync: {} as Record<string, { page: 'synced' | 'copy'; theme: 'synced' | 'copy'; locked: boolean }>,
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
    onResized: async () => () => undefined,
    onFocusChanged: async () => () => undefined,
  }),
}))
vi.mock('./ZenWidgetData', () => ({
  ZenWidgetData: () => <div data-garden-data-stub="" data-settings-id="zen-gardens.data" />,
}))
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(async () => undefined),
  openPath: vi.fn(async () => undefined),
  revealItemInDir: vi.fn(async () => undefined),
}))

function gardenJson(g: FakeGarden, i: number): unknown {
  return { id: g.id, name: g.name, index: i + 1, template: g.template, hasFile: true, createdAt: null, theme: g.theme }
}

function syncRow(g: FakeGarden, i: number): Record<string, unknown> {
  const st = h.sync[g.id] ?? { page: 'synced', theme: 'synced', locked: false }
  const part = (mode: string): unknown =>
    mode === 'copy' ? { mode, defaults: 'd-3f9a12c0b1e4d7a2', newerDefault: true, k2Version: '0.45.1' } : { mode, newerDefault: false }
  return {
    id: g.id,
    name: g.name,
    index: i + 1,
    page: part(st.page),
    theme: part(st.theme),
    ownChanges: ['layout'],
    undo: null,
    keepPrevious: { page: true, theme: true },
    themeName: 'basic',
    themeLabel: 'Basic',
    themeBuiltin: true,
    themeBase: { name: 'basic', label: 'Basic' },
    locked: st.locked,
  }
}

function fakeGet(route: string): unknown {
  if (route === 'zen/sync') {
    return { ok: true, liveDefaults: 'd-1111222233334444', k2Version: '0.45.5', previousDefaults: 'd-3f9a12c0b1e4d7a2', gardens: h.gardens.map(syncRow) }
  }
  if (route === 'zen/news') {
    return {
      ok: true,
      items: [{ id: 'catalog:diary', kind: 'catalog', short: 'diary', label: 'Diary', description: 'A journal.', template: 'k2.diary@1', version: 1 }],
      copiesWithNewerDefault: 1,
    }
  }
  if (route === 'zen/gardens') return { ok: true, setUp: true, gardens: h.gardens.map(gardenJson) }
  if (route === 'zen/status') return { ok: true, setUp: true, path: '/Users/r/.k2/zen', gardens: [] }
  if (route === 'zen/theme/list') {
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
  if (route === 'zen/templates') throw new Error('unknown zen route')
  throw new Error(`unexpected GET ${route}`)
}

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...mod,
    daemonCliGet: vi.fn(async (_scope: { hostKey: string }, route: string) => {
      if (route.startsWith('zen/')) return fakeGet(route)
      // The switch, off (a 0.46.3 page never asks for it).
      if (route === 'settings/get') return { gardenAccess: false }
      if (route === 'feedback/waiting-count') return { count: 0 }
      return {}
    }),
    daemonCliPost: vi.fn(async (_scope: { hostKey: string }, route: string) => {
      throw new Error(`the off golden posts nothing (POST ${route})`)
    }),
  }
})
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...mod,
    onZenChanged: () => () => undefined,
    subscribeToActiveState: () => () => undefined,
  }
})

import { act } from 'react'
import { cleanup, render, waitFor } from '@testing-library/react'
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenSettingsForTests, useZenSettingsStore } from '@/lib/zen/zen-settings'
import { __resetZenTemplatesForTests } from '@/lib/zen/zen-templates'
import { __resetZenSyncForTests } from '@/lib/zen/zen-sync'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { ZenGardensSection } from './ZenGardensSection'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

/** Present only on a tree with Garden access (a glob with no match is {}). */
const HAS_SWITCH = Object.keys(import.meta.glob('../../../lib/zen/garden-access-api.ts')).length === 1

const GOLDEN = join(process.cwd(), 'tests/golden/garden_access_off/renderer/settings_gardens_off.html')

/** One element per line, so a drift diffs readably. Never drops a byte of
 *  markup: only a newline is added before each tag. */
function pretty(html: string): string {
  return html.replace(/></g, '>\n<') + '\n'
}

function checkGolden(got: string): void {
  if (process.env.GARDEN_ACCESS_GOLDEN_BLESS === '1') {
    mkdirSync(dirname(GOLDEN), { recursive: true })
    writeFileSync(GOLDEN, got)
    console.log(`GOLDEN-BEGIN settings_gardens_off.html\n${Buffer.from(got).toString('base64')}\nGOLDEN-END`)
    return
  }
  if (!existsSync(GOLDEN)) throw new Error(`no golden at ${GOLDEN}; capture it from a base without Garden access`)
  expect(got).toBe(readFileSync(GOLDEN, 'utf8'))
}

beforeEach(() => {
  h.gardens = [
    { id: 'g-1', name: 'Garden 1', template: 'k2.texting@1', theme: null },
    { id: 'g-2', name: 'Garden 2', template: 'k2.diary@1', theme: 'paper' },
    { id: 'g-3', name: 'Mornings', template: 'k2.blank@1', theme: null },
  ]
  h.sync = {
    'g-2': { page: 'copy', theme: 'copy', locked: true },
  }
  window.location.hash = ''
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  __resetZenSettingsForTests()
  __resetZenTemplatesForTests()
  __resetZenSyncForTests()
  useToastStore.setState({ toasts: [] })
  useConfirmDialogStore.getState().close()
})

afterEach(() => {
  cleanup()
})

describe('Garden access off: Settings → Gardens is 0.46.3 plus the switch row', () => {
  it('settings_gardens_off', async () => {
    const { container } = render(<ZenGardensSection />)
    await waitFor(() => expect(useZenSettingsStore.getState().status).toBe('ready'))
    await waitFor(() => expect(container.querySelector('[data-zen-garden-lock="g-3"]')).not.toBeNull())
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })
    const rows = container.querySelectorAll('[data-garden-access-switch-row]')
    if (HAS_SWITCH) {
      // G0a item 1, the one named exception: the switch row, unchecked.
      await waitFor(() => expect(container.querySelectorAll('[data-garden-access-switch-row]').length).toBe(1))
      const row = container.querySelector('[data-garden-access-switch-row]') as HTMLElement
      const sw = row.querySelector('[role="switch"]')
      if (!sw) throw new Error(`the switch row has no switch\n${row.outerHTML}`)
      expect(sw.getAttribute('aria-checked')).toBe('false')
      expect(row.textContent).toContain('Garden access lists')
      row.remove()
    } else {
      expect(rows.length).toBe(0)
    }
    // Off never shows the master-detail page.
    expect(container.querySelector('[data-zen-gardens-section]')).not.toBeNull()
    checkGolden(pretty(container.innerHTML))
  })
})
