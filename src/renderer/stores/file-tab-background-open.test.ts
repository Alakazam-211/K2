// Thread file links, Option/Alt-click (0.46.1 RC, Rosson 2026-10-18): "open
// the tab but don't navigate to it".
//
//   - `openFileAsTab(path, { activate: false })` appends the tab unselected,
//     or finds the open one and leaves every column as it is;
//   - a plain `openFileAsTab(path)` still selects it;
//   - `parkFileTab` puts the tab in ANOTHER workspace's stored layout (no
//     switch): GET the layout with its revision, append, save with that
//     base; a 409 re-reads and retries; this window's cached copy and a
//     stashed live snapshot get the tab too.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'unavailable', reason: 'test env', port: null, token: null }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

/** The daemon's stored layouts, by `projectId:workspaceId`, with revisions. */
const daemon = vi.hoisted(() => ({
  layouts: new Map<string, { json: string; revision: number }>(),
  saves: [] as Array<{ projectId: string; workspaceId: string; layoutJson: string; baseRevision?: number }>,
  /** Conflicts to answer before accepting a save. */
  conflicts: 0,
  /** A peer's save that lands between our read and our write. */
  onConflict: null as null | (() => void),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(
      vi.fn(async (route: string, params?: Record<string, string>) => {
        if (route === 'workspace-layouts/load') {
          const row = daemon.layouts.get(`${params?.project_id}:${params?.workspace_id}`)
          return row ? { layoutJson: row.json, revision: row.revision } : { layoutJson: null, revision: 0 }
        }
        return []
      }),
    ),
    daemonCliGetText: vi.fn(async () => ''),
    daemonCliPost: primaryOnly(
      vi.fn(async (route: string, body: { projectId: string; workspaceId: string; layoutJson: string; baseRevision?: number }) => {
        if (route !== 'workspace-layouts/save') return {}
        daemon.saves.push(body)
        const key = `${body.projectId}:${body.workspaceId}`
        if (daemon.conflicts > 0) {
          daemon.conflicts--
          daemon.onConflict?.()
          throw new Error('409 layout_revision_conflict')
        }
        const cur = daemon.layouts.get(key)
        if (body.baseRevision !== undefined && cur && cur.revision !== body.baseRevision) {
          throw new Error('409 layout_revision_conflict')
        }
        const revision = (cur?.revision ?? 0) + 1
        daemon.layouts.set(key, { json: body.layoutJson, revision })
        return { success: true, revision }
      }),
    ),
    RecoveringError: class RecoveringError extends Error {},
  }
})
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'agent'),
  setChatSession: vi.fn(async () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
}))
vi.mock('./session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn(() => () => undefined),
  onSessionRemovedApp: vi.fn(() => () => undefined),
  onAppHello: vi.fn(() => () => undefined),
}))

import { useTabsStore, type FileViewerItemData, type SerializedLayout, type Tab } from './tabs'

function fileTab(id: string, filePath: string): Tab {
  const pgId = `pg-${id}`
  return {
    id,
    title: filePath.split('/').pop()!,
    mosaicTree: pgId,
    paneGroups: new Map([[pgId, { id: pgId, items: [{ id: `it-${id}`, type: 'file-viewer', data: { filePath }, pinned: false }], activeItemIndex: 0 }]]),
  } as Tab
}

function filesOf(tabs: Tab[]): string[] {
  return tabs.flatMap((t) =>
    Array.from(t.paneGroups.values()).flatMap((pg) =>
      pg.items.filter((i) => i.type === 'file-viewer').map((i) => (i.data as FileViewerItemData).filePath),
    ),
  )
}

function serializedFiles(layout: SerializedLayout): string[] {
  return layout.tabs.flatMap((t) =>
    Object.values(t.paneGroups).flatMap((pg) =>
      pg.items.filter((i) => i.type === 'file-viewer').map((i) => (i as { filePath?: string }).filePath ?? ''),
    ),
  )
}

const SEW_LAYOUT: SerializedLayout = {
  version: 2,
  tabs: [
    {
      id: 'sew-readme',
      title: 'README.md',
      mosaicTree: 'pg-sew',
      paneGroups: { 'pg-sew': { id: 'pg-sew', items: [{ id: 'i1', type: 'file-viewer', filePath: '/sew/README.md' }], activeItemIndex: 0 } },
    },
  ],
}

beforeEach(() => {
  mem.clear()
  daemon.layouts.clear()
  daemon.saves.length = 0
  daemon.conflicts = 0
  daemon.onConflict = null
  useTabsStore.room.__test.resetLayoutSync()
  useTabsStore.setState({
    tabs: [fileTab('chat', '/k2/notes.md'), fileTab('b', '/k2/b.ts')],
    activeTabId: 'chat',
    extraGroups: [],
    splitCount: 1,
    activeGroupIndex: 0,
    activeWorkspaceKey: 'p-k2:w-k2',
    activeProjectId: 'p-k2',
    activeWorkspaceId: 'w-k2',
    workspaceLayouts: {},
    backgroundWorkspaces: {},
  })
})

describe('openFileAsTab in the open workspace', () => {
  it('activate: false appends the tab without selecting it', () => {
    const result = useTabsStore.getState().openFileAsTab('/k2/src/a.ts', { activate: false })
    const st = useTabsStore.getState()
    expect(result).toBe('opened')
    expect(filesOf(st.tabs)).toEqual(['/k2/notes.md', '/k2/b.ts', '/k2/src/a.ts'])
    expect(st.activeTabId).toBe('chat')
    expect(st.activeGroupIndex).toBe(0)
  })

  it('activate: false on a file already open changes nothing', () => {
    const before = useTabsStore.getState()
    const result = useTabsStore.getState().openFileAsTab('/k2/b.ts', { activate: false })
    const st = useTabsStore.getState()
    expect(result).toBe('existing')
    expect(st.tabs).toBe(before.tabs)
    expect(st.activeTabId).toBe('chat')
  })

  it('activate: false on a file open in a split column leaves the focused column', () => {
    useTabsStore.setState({ extraGroups: [{ tabs: [fileTab('c', '/k2/c.ts')], activeTabId: 'c' }], splitCount: 2 })
    const result = useTabsStore.getState().openFileAsTab('/k2/c.ts', { activate: false })
    expect(result).toBe('existing')
    expect(useTabsStore.getState().activeGroupIndex).toBe(0)
  })

  it('a plain open still selects the tab', () => {
    expect(useTabsStore.getState().openFileAsTab('/k2/src/a.ts')).toBe('opened')
    const st = useTabsStore.getState()
    expect(st.activeTabId).toBe(st.tabs[2].id)
    useTabsStore.getState().openFileAsTab('/k2/b.ts')
    expect(useTabsStore.getState().activeTabId).toBe('b')
  })
})

describe('parkFileTab: another workspace, no switch', () => {
  it('adds the tab to the stored layout with its revision, and to the cached copy', async () => {
    daemon.layouts.set('p-sew:w-sew', { json: JSON.stringify(SEW_LAYOUT), revision: 7 })
    const result = await useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/src/main.rs')
    expect(result).toBe('opened')
    expect(daemon.saves).toHaveLength(1)
    expect(daemon.saves[0].baseRevision).toBe(7)
    const stored = JSON.parse(daemon.layouts.get('p-sew:w-sew')!.json) as SerializedLayout
    expect(serializedFiles(stored)).toEqual(['/sew/README.md', '/sew/src/main.rs'])
    const st = useTabsStore.getState()
    expect(serializedFiles(st.workspaceLayouts['p-sew:w-sew'])).toEqual(['/sew/README.md', '/sew/src/main.rs'])
    // The open workspace and its selection never moved.
    expect(st.activeWorkspaceKey).toBe('p-k2:w-k2')
    expect(st.activeTabId).toBe('chat')
    expect(filesOf(st.tabs)).toEqual(['/k2/notes.md', '/k2/b.ts'])
  })

  it('a workspace never opened (no stored layout) gets a layout with the tab', async () => {
    const result = await useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/src/main.rs')
    expect(result).toBe('opened')
    const stored = JSON.parse(daemon.layouts.get('p-sew:w-sew')!.json) as SerializedLayout
    expect(serializedFiles(stored)).toEqual(['/sew/src/main.rs'])
  })

  it('a file already in that layout is left alone', async () => {
    daemon.layouts.set('p-sew:w-sew', { json: JSON.stringify(SEW_LAYOUT), revision: 7 })
    expect(await useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/README.md')).toBe('existing')
    expect(daemon.saves).toHaveLength(0)
  })

  it('a 409 (a peer saved in between) re-reads the layout and keeps the peer\'s tab', async () => {
    daemon.layouts.set('p-sew:w-sew', { json: JSON.stringify(SEW_LAYOUT), revision: 7 })
    daemon.conflicts = 1
    daemon.onConflict = () => {
      const peer: SerializedLayout = {
        ...SEW_LAYOUT,
        tabs: [
          ...SEW_LAYOUT.tabs,
          {
            id: 'peer',
            title: 'peer.ts',
            mosaicTree: 'pg-peer',
            paneGroups: { 'pg-peer': { id: 'pg-peer', items: [{ id: 'ip', type: 'file-viewer', filePath: '/sew/peer.ts' }], activeItemIndex: 0 } },
          },
        ],
      }
      daemon.layouts.set('p-sew:w-sew', { json: JSON.stringify(peer), revision: 8 })
    }
    expect(await useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/src/main.rs')).toBe('opened')
    expect(daemon.saves.map((s) => s.baseRevision)).toEqual([7, 8])
    const stored = JSON.parse(daemon.layouts.get('p-sew:w-sew')!.json) as SerializedLayout
    expect(serializedFiles(stored)).toEqual(['/sew/README.md', '/sew/peer.ts', '/sew/src/main.rs'])
  })

  it('gives up loudly when the layout keeps changing', async () => {
    daemon.conflicts = 10
    await expect(useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/a.rs')).rejects.toThrow(/kept changing/)
  })

  it('a stashed (live) workspace gets the tab in its snapshot too, unselected', async () => {
    daemon.layouts.set('p-sew:w-sew', { json: JSON.stringify(SEW_LAYOUT), revision: 3 })
    useTabsStore.setState({
      backgroundWorkspaces: {
        'p-sew:w-sew': { tabs: [fileTab('sew-readme', '/sew/README.md')], extraGroups: [], splitCount: 1, activeGroupIndex: 0, activeTabId: 'sew-readme' },
      },
    })
    expect(await useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/src/main.rs')).toBe('opened')
    const snap = useTabsStore.getState().backgroundWorkspaces['p-sew:w-sew']
    expect(filesOf(snap.tabs)).toEqual(['/sew/README.md', '/sew/src/main.rs'])
    expect(snap.activeTabId).toBe('sew-readme')
  })

  it('a file already in the stashed snapshot is left alone, without a save', async () => {
    useTabsStore.setState({
      backgroundWorkspaces: {
        'p-sew:w-sew': { tabs: [fileTab('x', '/sew/a.rs')], extraGroups: [], splitCount: 1, activeGroupIndex: 0, activeTabId: 'x' },
      },
    })
    expect(await useTabsStore.room.parkFileTab('p-sew', 'w-sew', '/sew/a.rs')).toBe('existing')
    expect(daemon.saves).toHaveLength(0)
  })

  it('the open workspace goes through openFileAsTab, unselected', async () => {
    expect(await useTabsStore.room.parkFileTab('p-k2', 'w-k2', '/k2/new.ts')).toBe('opened')
    const st = useTabsStore.getState()
    expect(filesOf(st.tabs)).toContain('/k2/new.ts')
    expect(st.activeTabId).toBe('chat')
    expect(daemon.saves.filter((s) => s.workspaceId === 'w-sew')).toHaveLength(0)
  })
})
