// Desktop tab (prd-remote-desktop-tab-v1 §8) in the tabs store, fail loud:
//  - `openDesktopTab` focuses the server's Desktop tab (one machine per
//    server) or opens one in the column asked for;
//  - a Desktop item saves as an empty Browser item plus the `desktop`
//    marker and restores as Desktop on a new client; an older client
//    restores the same saved item as an empty Browser tab, and its re-save
//    (no marker) restores as an empty Browser tab, never a file tab; no
//    schema bump;
//  - `setDesktopItemState` stamps display / fit / view only;
//  - `open_desktop` (`k2 desktop open`) opens the tab only when `for` names
//    this client, on a server that lists `remote-desktop-v1`.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

type DesktopEvt = { kind: 'open_desktop'; desktop: string; display: string | null; for: string }
const bus = vi.hoisted(() => ({ handlers: new Map<string, Set<(e: DesktopEvt) => void>>() }))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onOpenDesktop: (scope: { id: string }, fn: (e: DesktopEvt) => void) => {
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
  initOpenDesktopTabs,
  registerActiveProjectIdGetter,
  registerProjectsPathIndex,
  useTabsStore,
  type DesktopItemData,
  type SerializedLayout,
  type Tab,
} from './tabs'

function fire(scopeId: string, forField: string, display: string | null = null): void {
  for (const fn of bus.handlers.get(scopeId) ?? []) fn({ kind: 'open_desktop', desktop: 'console', display, for: forField })
}

function items(tabs: Tab[] = useTabsStore.getState().tabs): Array<{ id: string; type: string; data: unknown }> {
  return tabs.flatMap((t) => Array.from(t.paneGroups.values()).flatMap((pg) => pg.items))
}

function desktopTabs(tabs: Tab[] = useTabsStore.getState().tabs): Tab[] {
  return tabs.filter((t) => Array.from(t.paneGroups.values()).some((pg) => pg.items.some((i) => i.type === 'desktop')))
}

beforeEach(() => {
  bus.handlers.clear()
  __resetServerScopesForTests()
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
  registerProjectsPathIndex(() => [{ id: 'p1', path: '/srv/alpha', primaryWorkspaceId: 'w1' }])
  registerActiveProjectIdGetter(() => 'p1')
})

afterEach(() => {
  registerProjectsPathIndex(() => [])
  registerActiveProjectIdGetter(() => null)
  __resetServerScopesForTests()
})

describe('openDesktopTab', () => {
  it('opens one Desktop tab, then focuses it instead of opening another', () => {
    const a = useTabsStore.getState().openDesktopTab()
    useTabsStore.getState().addTab('/srv/alpha')
    expect(useTabsStore.getState().activeTabId).not.toBe(a)
    const b = useTabsStore.getState().openDesktopTab()
    const s = useTabsStore.getState()
    expect(b).toBe(a)
    expect(s.activeTabId).toBe(a)
    expect(desktopTabs()).toHaveLength(1)
    expect(s.tabs.find((t) => t.id === a)?.title).toBe('Desktop')
    expect(items(desktopTabs()).map((i) => i.data)).toEqual([{ desktopId: 'console' }])
  })

  it('setDesktopItemState stamps the item; the same value changes nothing', () => {
    const tabId = useTabsStore.getState().openDesktopTab()
    const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId)!
    const [pgId, pg] = Array.from(tab.paneGroups.entries())[0]
    useTabsStore.getState().setDesktopItemState(tabId, pgId, pg.items[0].id, { displayId: '2', fit: '1:1' })
    const after = useTabsStore.getState().tabs
    expect(items(after)[0].data as DesktopItemData).toEqual({ desktopId: 'console', displayId: '2', fit: '1:1' })
    useTabsStore.getState().setDesktopItemState(tabId, pgId, pg.items[0].id, { fit: '1:1' })
    expect(useTabsStore.getState().tabs).toBe(after)
  })
})

describe('serialization (§8 mixed-version guard)', () => {
  it('saves as an empty Browser item plus the marker and restores as Desktop', () => {
    const tabId = useTabsStore.getState().openDesktopTab()
    const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId)!
    const [pgId, pg] = Array.from(tab.paneGroups.entries())[0]
    const itemId = pg.items[0].id
    useTabsStore.getState().setDesktopItemState(tabId, pgId, itemId, { displayId: '1', viewOnly: true })
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    expect(layout.version).toBe(LAYOUT_SCHEMA_VERSION)
    const saved = layout.tabs.flatMap((t) => Object.values(t.paneGroups).flatMap((g) => g.items))
    expect(saved).toEqual([
      { id: itemId, type: 'browser', url: '', title: 'Desktop', desktop: { v: 1, desktopId: 'console', displayId: '1', viewOnly: true } },
    ])

    useTabsStore.setState({ tabs: [], activeTabId: null })
    useTabsStore.getState().restoreLayout(layout, '/srv/alpha')
    const restored = items()
    expect(restored).toHaveLength(1)
    expect(restored[0].id).toBe(itemId)
    expect(restored[0].type).toBe('desktop')
    expect(restored[0].data).toEqual({ desktopId: 'console', displayId: '1', viewOnly: true })
  })

  it('the saved item is a plain empty Browser item to an older client, and its re-save restores as an empty Browser tab', () => {
    useTabsStore.getState().openDesktopTab()
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    const saved = layout.tabs.flatMap((t) => Object.values(t.paneGroups).flatMap((g) => g.items))
    // What an older client reads: type browser, url '' (the marker is a
    // field it ignores).
    expect(saved.map((i) => [i.type, (i as { url?: string }).url])).toEqual([['browser', '']])
    // Its re-save drops the field it doesn't know.
    for (const t of layout.tabs) {
      for (const g of Object.values(t.paneGroups)) {
        for (const item of g.items) delete (item as { desktop?: unknown }).desktop
      }
    }
    useTabsStore.setState({ tabs: [], activeTabId: null })
    useTabsStore.getState().restoreLayout(layout, '/srv/alpha')
    const restored = items()
    expect(restored.map((i) => i.type)).toEqual(['browser'])
    expect((restored[0].data as { url: string }).url).toBe('')
  })
})

describe('open_desktop (k2 desktop open)', () => {
  it('opens (or focuses) the Desktop tab only when `for` names this client', () => {
    noteServerVersion('local', '0.46.11', ['remote-desktop-v1'])
    const off = initOpenDesktopTabs()
    const id = primaryScope().id
    // This window is this computer's app (the owner token): `owner` only.
    fire(id, 'rosson')
    expect(desktopTabs()).toHaveLength(0)
    fire(id, 'owner', '2')
    expect(desktopTabs()).toHaveLength(1)
    expect(items(desktopTabs())[0].data).toEqual({ desktopId: 'console', displayId: '2' })
    fire(id, 'owner')
    expect(desktopTabs()).toHaveLength(1)
    off()
  })

  it('opens nothing on a server without remote-desktop-v1', () => {
    noteServerVersion('local', '0.46.11', ['tickets-list-all'])
    const off = initOpenDesktopTabs()
    fire(primaryScope().id, 'owner')
    expect(desktopTabs()).toHaveLength(0)
    off()
  })
})
