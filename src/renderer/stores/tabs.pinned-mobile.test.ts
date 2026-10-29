// The pinned Mobile tab (Rosson 2026-10-29) in the tabs store, fail loud:
//  - "Show the Mobile tab" (`ProjectPathEntry.mobileTab`: null = auto, true,
//    false) decides it on a workspace restore: auto / on make it where the
//    server can run a phone (`mobile-sim-v1`), off never does, and a server
//    with no phone never gets one;
//  - it sits after Chat, Inbox and the pinned Design tab, cannot be closed or
//    user-pinned, and saves / restores with its flag;
//  - turning the setting off removes it; on promotes an ordinary Mobile tab
//    instead of opening a second view of the same device;
//  - + → Mobile / ⌘⇧M / open_mobile while it is off gives an ordinary,
//    closable, unpinned Mobile tab and changes no setting.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'k2so_agents_list') return [{ name: 'app-agent', agentType: 'custom' }]
    if (cmd === 'daemon_ws_url') return { state: 'unavailable', reason: 'test env', port: null, token: null }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return { ...actual, daemonCliPost: vi.fn(async () => ({ ok: true })) }
})

import { daemonCliPost } from '@/lib/daemon-cli'
import { __resetServerScopesForTests, noteServerVersion } from '@/kessel/server-scope'
import {
  registerProjectsPathIndex,
  useTabsStore,
  withPinnedMobileTab,
  type ProjectPathEntry,
  type SerializedLayout,
  type Tab,
} from './tabs'

let projectIndex: ProjectPathEntry[] = []

function reset(): void {
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

function firstItem(tab: Tab): { id: string; type: string; data: unknown } {
  const item = Array.from(tab.paneGroups.values())[0]?.items[0]
  if (!item) throw new Error(`tab ${tab.id} has no item`)
  return item
}

/** `chat` / `inbox` / `pinned-design` / `pinned-mobile` / item type, per tab. */
function strip(tabs: Tab[] = useTabsStore.getState().tabs): string[] {
  return tabs.map((t) => {
    const item = firstItem(t)
    if (t.isSystemAgent) return (item.data as { section?: string }).section ?? 'system'
    if (t.isPinnedDesign) return `pinned-${item.type}`
    if (t.isPinnedMobile) return `pinned-${item.type}`
    return item.type
  })
}

function project(path: string, mobileTab: boolean | null, designMode = false): ProjectPathEntry {
  return { id: `id-${path}`, path, primaryWorkspaceId: null, designMode, mobileTab }
}

async function restoreWorkspace(path: string): Promise<void> {
  await useTabsStore.room.ensurePinnedAgentTabForMode('custom', path)
}

/** This computer's daemon says whether it can run a phone. */
function phone(can: boolean): void {
  noteServerVersion('local', '0.46.7', can ? ['mobile-sim-v1'] : ['tickets-list-all'])
}

beforeEach(() => {
  reset()
  __resetServerScopesForTests()
  projectIndex = []
  registerProjectsPathIndex(() => projectIndex)
  vi.mocked(daemonCliPost).mockClear()
})

afterEach(() => {
  registerProjectsPathIndex(() => [])
  __resetServerScopesForTests()
  reset()
})

describe('the pinned Mobile tab follows "Show the Mobile tab"', () => {
  it('auto on a server that can run a phone: created after Chat, Inbox and Design', async () => {
    phone(true)
    projectIndex = [project('/w/app', null, true)]
    useTabsStore.getState().addTab('/w/app', { title: 'T' })
    await restoreWorkspace('/w/app')
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design', 'pinned-mobile', 'terminal'])
    // Idempotent: a second ensure changes nothing (no save).
    const before = useTabsStore.getState().tabs
    useTabsStore.getState().ensurePinnedMobileTab('/w/app')
    expect(useTabsStore.getState().tabs).toBe(before)
  })

  it('on: created; off: never created', async () => {
    phone(true)
    projectIndex = [project('/w/on', true), project('/w/off', false)]
    await restoreWorkspace('/w/on')
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-mobile'])
    reset()
    await restoreWorkspace('/w/off')
    expect(strip()).toEqual(['chat', 'inbox'])
  })

  it('a server that cannot run a phone never gets one, whatever the setting', async () => {
    phone(false)
    projectIndex = [project('/w/a', null), project('/w/b', true)]
    await restoreWorkspace('/w/a')
    expect(strip()).toEqual(['chat', 'inbox'])
    reset()
    await restoreWorkspace('/w/b')
    expect(strip()).toEqual(['chat', 'inbox'])
  })

  it('auto with no phone known never REMOVES one already there (features not read yet)', () => {
    phone(false)
    projectIndex = [project('/w/a', null)]
    useTabsStore.setState({ tabs: withPinnedMobileTab([], true, true) })
    useTabsStore.getState().ensurePinnedMobileTab('/w/a')
    expect(strip()).toEqual(['pinned-mobile'])
  })

  it('a workspace the room does not know is left alone (no guess)', () => {
    phone(true)
    useTabsStore.getState().ensurePinnedMobileTab('/w/unknown')
    expect(useTabsStore.getState().tabs).toHaveLength(0)
  })

  it('turning it off removes it and lands on the first tab; on brings it back', async () => {
    phone(true)
    projectIndex = [project('/w/app', null)]
    await restoreWorkspace('/w/app')
    const mobileId = useTabsStore.getState().tabs[2].id
    useTabsStore.getState().setActiveTabInGroup(0, mobileId)

    useTabsStore.getState().ensurePinnedMobileTab('/w/app', false)
    let s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox'])
    expect(s.activeTabId).toBe(s.tabs[0].id)

    useTabsStore.getState().ensurePinnedMobileTab('/w/app', true)
    s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-mobile'])
    expect(s.tabs.filter((t) => t.isPinnedMobile)).toHaveLength(1)
  })

  it('cannot be closed or user-pinned', async () => {
    phone(true)
    projectIndex = [project('/w/app', null)]
    await restoreWorkspace('/w/app')
    const mobile = useTabsStore.getState().tabs[2]
    expect(mobile.isPinnedMobile).toBe(true)
    useTabsStore.getState().removeTab(mobile.id)
    useTabsStore.getState().togglePinTab(mobile.id)
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-mobile'])
    expect(useTabsStore.getState().tabs[2].isPinnedFile).toBeUndefined()
  })

  it('saves its flag and restores as the pinned Mobile tab', () => {
    useTabsStore.setState({ tabs: withPinnedMobileTab([], true, true) })
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    expect(layout.tabs[0].isPinnedMobile).toBe(true)
    reset()
    useTabsStore.getState().restoreLayout(layout, '/w/app')
    expect(strip()).toEqual(['pinned-mobile'])
  })

  it('two pinned Mobile tabs (two windows each made one) keep one; the other stays as an ordinary tab', () => {
    const one = withPinnedMobileTab([], true, true)
    const two = withPinnedMobileTab([], true, true)
    const merged = withPinnedMobileTab([...one, ...two], true, true)
    expect(strip(merged)).toEqual(['pinned-mobile', 'mobile'])
  })
})

describe('opening Mobile while "Show the Mobile tab" is off', () => {
  it('gives an ordinary, closable, unpinned Mobile tab and writes no setting', async () => {
    phone(true)
    projectIndex = [project('/w/app', false)]
    await restoreWorkspace('/w/app')
    expect(strip()).toEqual(['chat', 'inbox'])

    const id = useTabsStore.getState().openMobileTab()
    let s = useTabsStore.getState()
    const tab = s.tabs.find((t) => t.id === id)
    if (!tab) throw new Error('no Mobile tab opened')
    expect(tab.isPinnedMobile).toBeUndefined()
    expect(tab.isPinnedFile).toBeUndefined()
    expect(strip()).toEqual(['chat', 'inbox', 'mobile'])
    // The off setting does not take an ordinary tab away.
    useTabsStore.getState().ensurePinnedMobileTab('/w/app')
    expect(strip()).toEqual(['chat', 'inbox', 'mobile'])
    // It closes like any tab.
    useTabsStore.getState().removeTab(id)
    s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox'])
    expect(vi.mocked(daemonCliPost)).not.toHaveBeenCalled()
  })

  it('turning it on pins that ordinary Mobile tab (one device view), not a second one', async () => {
    phone(true)
    projectIndex = [project('/w/app', false)]
    await restoreWorkspace('/w/app')
    const id = useTabsStore.getState().openMobileTab()
    useTabsStore.getState().ensurePinnedMobileTab('/w/app', true)
    const s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-mobile'])
    expect(s.tabs[2].id).toBe(id)
  })

  it('openMobileTab focuses the pinned Mobile tab when there is one', async () => {
    phone(true)
    projectIndex = [project('/w/app', null)]
    await restoreWorkspace('/w/app')
    const pinnedId = useTabsStore.getState().tabs[2].id
    expect(useTabsStore.getState().openMobileTab()).toBe(pinnedId)
    expect(useTabsStore.getState().tabs).toHaveLength(3)
    expect(useTabsStore.getState().activeTabId).toBe(pinnedId)
  })
})
