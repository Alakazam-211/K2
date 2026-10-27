// Mobile tab (prd-mobile-dev-v1 §2.3; vs-live M-A7, M-A10, M-A13) in the
// tabs store, fail loud:
//  - `openMobileTab` focuses the workspace's Mobile tab (one device per
//    workspace) or opens one in the column asked for;
//  - a Mobile item saves as an empty Browser item plus the `mobile` marker,
//    restores as Mobile on a new client and as an empty Browser tab when an
//    older client dropped the marker; no schema bump;
//  - `open_mobile` (`k2 sim boot`) opens the tab only in a room that shows
//    that workspace on a server that lists `mobile-sim-v1`.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

type MobileEvt = { kind: 'open_mobile'; workspacePath: string }
const bus = vi.hoisted(() => ({ handlers: new Map<string, Set<(e: MobileEvt) => void>>() }))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onOpenMobile: (scope: { id: string }, fn: (e: MobileEvt) => void) => {
      const set = bus.handlers.get(scope.id) ?? new Set()
      set.add(fn)
      bus.handlers.set(scope.id, set)
      return () => set.delete(fn)
    },
  }
})

import { __resetServerScopesForTests, noteServerVersion, primaryScope } from '@/kessel/server-scope'
import {
  LAYOUT_SCHEMA_VERSION,
  initOpenMobileTabs,
  registerActiveProjectIdGetter,
  registerProjectsPathIndex,
  useTabsStore,
  type MobileItemData,
  type SerializedLayout,
  type Tab,
} from './tabs'

function fire(scopeId: string, workspacePath: string): void {
  for (const fn of bus.handlers.get(scopeId) ?? []) fn({ kind: 'open_mobile', workspacePath })
}

function items(tabs: Tab[] = useTabsStore.getState().tabs): Array<{ id: string; type: string; data: unknown }> {
  return tabs.flatMap((t) => Array.from(t.paneGroups.values()).flatMap((pg) => pg.items))
}

function mobileTabs(tabs: Tab[] = useTabsStore.getState().tabs): Tab[] {
  return tabs.filter((t) => Array.from(t.paneGroups.values()).some((pg) => pg.items.some((i) => i.type === 'mobile')))
}

let active: string | null = 'p1'

beforeEach(() => {
  bus.handlers.clear()
  __resetServerScopesForTests()
  active = 'p1'
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
  registerProjectsPathIndex(() => [
    { id: 'p1', path: '/srv/alpha', primaryWorkspaceId: 'w1' },
    { id: 'p2', path: '/srv/beta', primaryWorkspaceId: 'w2' },
  ])
  registerActiveProjectIdGetter(() => active)
})

afterEach(() => {
  registerProjectsPathIndex(() => [])
  registerActiveProjectIdGetter(() => null)
  __resetServerScopesForTests()
})

describe('openMobileTab', () => {
  it('opens one Mobile tab, then focuses it instead of opening another', () => {
    const a = useTabsStore.getState().openMobileTab()
    useTabsStore.getState().addTab('/srv/alpha')
    expect(useTabsStore.getState().activeTabId).not.toBe(a)
    const b = useTabsStore.getState().openMobileTab()
    const s = useTabsStore.getState()
    expect(b).toBe(a)
    expect(s.activeTabId).toBe(a)
    expect(mobileTabs()).toHaveLength(1)
    expect(s.tabs.find((t) => t.id === a)?.title).toBe('Mobile')
    expect(items(mobileTabs()).map((i) => i.data)).toEqual([{}])
  })

  it('finds a Mobile tab in another column and focuses that column', () => {
    useTabsStore.getState().addTab('/srv/alpha')
    const id = useTabsStore.getState().openMobileTab()
    useTabsStore.setState((s) => ({
      tabs: s.tabs.filter((t) => t.id !== id),
      extraGroups: [{ tabs: s.tabs.filter((t) => t.id === id), activeTabId: null }],
      splitCount: 2,
      activeGroupIndex: 0,
    }))
    expect(useTabsStore.getState().openMobileTab()).toBe(id)
    const s = useTabsStore.getState()
    expect(s.activeGroupIndex).toBe(1)
    expect(s.extraGroups[0].activeTabId).toBe(id)
    expect(mobileTabs(s.tabs)).toHaveLength(0)
  })

  it('setMobileItemDevice stamps the item; the same value changes nothing', () => {
    const tabId = useTabsStore.getState().openMobileTab()
    const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId)!
    const [pgId, pg] = Array.from(tab.paneGroups.entries())[0]
    useTabsStore.getState().setMobileItemDevice(tabId, pgId, pg.items[0].id, 'pixel-9')
    const after = useTabsStore.getState().tabs
    expect(items(after)[0].data as MobileItemData).toEqual({ deviceType: 'pixel-9' })
    useTabsStore.getState().setMobileItemDevice(tabId, pgId, pg.items[0].id, 'pixel-9')
    expect(useTabsStore.getState().tabs).toBe(after)
  })
})

describe('serialization (M-A7 mixed-version guard)', () => {
  it('saves as an empty Browser item plus the marker and restores as Mobile', () => {
    const tabId = useTabsStore.getState().openMobileTab()
    const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId)!
    const [pgId, pg] = Array.from(tab.paneGroups.entries())[0]
    const itemId = pg.items[0].id
    useTabsStore.getState().setMobileItemDevice(tabId, pgId, itemId, 'pixel-9')
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    expect(layout.version).toBe(LAYOUT_SCHEMA_VERSION)
    const saved = layout.tabs.flatMap((t) => Object.values(t.paneGroups).flatMap((g) => g.items))
    expect(saved).toEqual([{ id: itemId, type: 'browser', url: '', title: 'Mobile', mobile: { v: 1, deviceType: 'pixel-9' } }])

    useTabsStore.setState({ tabs: [], activeTabId: null })
    useTabsStore.getState().restoreLayout(layout, '/srv/alpha')
    const restored = items()
    expect(restored).toHaveLength(1)
    expect(restored[0].id).toBe(itemId)
    expect(restored[0].type).toBe('mobile')
    expect(restored[0].data).toEqual({ deviceType: 'pixel-9' })
  })

  it('an older client re-save (no marker) restores as an empty Browser tab, never a file tab', () => {
    useTabsStore.getState().openMobileTab()
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    for (const t of layout.tabs) {
      for (const g of Object.values(t.paneGroups)) {
        for (const item of g.items) delete (item as { mobile?: unknown }).mobile
      }
    }
    useTabsStore.setState({ tabs: [], activeTabId: null })
    useTabsStore.getState().restoreLayout(layout, '/srv/alpha')
    const restored = items()
    expect(restored.map((i) => i.type)).toEqual(['browser'])
    expect((restored[0].data as { url: string }).url).toBe('')
  })
})

describe('open_mobile (M-A10)', () => {
  it('opens (or focuses) the Mobile tab only for the workspace this room shows', () => {
    const off = initOpenMobileTabs()
    const id = primaryScope().id
    fire(id, '/srv/beta')
    expect(mobileTabs()).toHaveLength(0)
    fire(id, '/srv/alpha')
    fire(id, '/srv/alpha/')
    expect(mobileTabs()).toHaveLength(1)
    const s = useTabsStore.getState()
    expect(s.tabs.find((t) => t.id === s.activeTabId)?.title).toBe('Mobile')
    off()
    useTabsStore.setState({ tabs: [], activeTabId: null })
    fire(id, '/srv/alpha')
    expect(mobileTabs()).toHaveLength(0)
  })

  it('a server without mobile-sim-v1 never opens a Mobile tab', () => {
    // This computer's daemon reported its features without the key.
    noteServerVersion('local', '0.46.6', ['tickets-list-all'])
    expect(primaryScope().serverSupports('mobile-sim-v1')).toBe(false)
    const off = initOpenMobileTabs()
    fire(primaryScope().id, '/srv/alpha')
    expect(mobileTabs()).toHaveLength(0)
    noteServerVersion('local', '0.46.6', ['mobile-sim-v1'])
    fire(primaryScope().id, '/srv/alpha')
    expect(mobileTabs()).toHaveLength(1)
    off()
  })
})
