// New documents (Cmd+N, the tab + menu): the tab's name comes from the daemon,
// which checks the disk, and two quick presses never share one.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return { ...actual, daemonCliPost: (...args: unknown[]) => daemonCliPost(...args) }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { HostSwitchedError } from '@/lib/daemon-cli'
import { useTabsStore, type FileViewerItemData, type Tab } from './tabs'

const blank = { tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, backgroundWorkspaces: {} }

function fileOf(tab: Tab): string {
  const pg = Array.from(tab.paneGroups.values())[0]
  return (pg.items[0].data as FileViewerItemData).filePath
}

/** The daemon, with these files on disk. */
function daemonWith(onDisk: string[]): void {
  daemonCliPost.mockImplementation(async (_scope: unknown, route: string, body: { taken?: string[] }) => {
    if (route !== 'fs/untitled-name') return { success: true }
    const taken = new Set([...onDisk, ...(body.taken ?? [])])
    let name = 'Untitled'
    for (let n = 2; taken.has(name); n += 1) name = `Untitled-${n}`
    return { name }
  })
}

beforeEach(() => {
  daemonCliPost.mockReset()
  useTabsStore.setState(blank)
})

afterEach(() => {
  useTabsStore.setState(blank)
})

describe('openUntitledDocument', () => {
  it('skips a file on disk that no tab knows about', async () => {
    daemonWith(['Untitled'])
    await useTabsStore.getState().openUntitledDocument('/ws')
    const [tab] = useTabsStore.getState().tabs
    expect(tab.title).toBe('Untitled-2')
    expect(fileOf(tab)).toBe('/ws/Untitled-2')
  })

  it('tells the daemon which names open tabs hold', async () => {
    daemonWith([])
    await useTabsStore.getState().openUntitledDocument('/ws')
    await useTabsStore.getState().openUntitledDocument('/ws')
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(['Untitled', 'Untitled-2'])
    expect(daemonCliPost.mock.calls[1].slice(1)).toEqual(['fs/untitled-name', { dir: '/ws', taken: ['Untitled'] }])
  })

  it('three quick presses, with none awaited, make three names', async () => {
    daemonWith([])
    const store = useTabsStore.getState()
    await Promise.all([store.openUntitledDocument('/ws'), store.openUntitledDocument('/ws'), store.openUntitledDocument('/ws')])
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(['Untitled', 'Untitled-2', 'Untitled-3'])
  })

  it('a failed press does not stop the next one', async () => {
    daemonCliPost.mockRejectedValueOnce(new HostSwitchedError('a', 'b'))
    daemonWith([])
    const store = useTabsStore.getState()
    await Promise.all([store.openUntitledDocument('/ws'), store.openUntitledDocument('/ws')])
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(['Untitled'])
  })

  it('opens no tab when the window has left the host', async () => {
    daemonCliPost.mockRejectedValue(new HostSwitchedError('a', 'b'))
    await useTabsStore.getState().openUntitledDocument('/ws')
    expect(useTabsStore.getState().tabs).toEqual([])
  })

  it('opens no tab in a workspace the person has since switched to', async () => {
    let answer: (v: { name: string }) => void = () => undefined
    daemonCliPost.mockImplementation(() => new Promise((resolve) => { answer = resolve }))
    const pending = useTabsStore.getState().openUntitledDocument('/ws')
    // The press runs on the queue's next tick.
    await vi.waitFor(() => expect(daemonCliPost).toHaveBeenCalled())
    useTabsStore.setState({ activeWorkspaceKey: 'other:other' })
    answer({ name: 'Untitled' })
    await pending
    expect(useTabsStore.getState().tabs).toEqual([])
    useTabsStore.setState({ activeWorkspaceKey: null })
  })
})

describe('retargetUntitledFile', () => {
  it('moves the tab and its title to the file the first save made', async () => {
    daemonWith([])
    await useTabsStore.getState().openUntitledDocument('/ws')
    const id = useTabsStore.getState().tabs[0].id
    useTabsStore.getState().retargetUntitledFile(id, '/ws/Untitled', '/ws/Untitled-2')
    const [tab] = useTabsStore.getState().tabs
    expect(tab.title).toBe('Untitled-2')
    expect(fileOf(tab)).toBe('/ws/Untitled-2')
  })

  it('keeps a title the person changed', async () => {
    daemonWith([])
    await useTabsStore.getState().openUntitledDocument('/ws')
    const id = useTabsStore.getState().tabs[0].id
    useTabsStore.setState({ tabs: useTabsStore.getState().tabs.map((t) => ({ ...t, title: 'Plans' })) })
    useTabsStore.getState().retargetUntitledFile(id, '/ws/Untitled', '/ws/Untitled-2')
    const [tab] = useTabsStore.getState().tabs
    expect(tab.title).toBe('Plans')
    expect(fileOf(tab)).toBe('/ws/Untitled-2')
  })

  it('finds a tab in a column, and one in a left workspace’s snapshot', async () => {
    daemonWith([])
    await useTabsStore.getState().openUntitledDocument('/ws')
    const live = useTabsStore.getState().tabs
    const id = live[0].id

    useTabsStore.setState({ tabs: [], extraGroups: [{ tabs: live, activeTabId: id }], splitCount: 2 })
    useTabsStore.getState().retargetUntitledFile(id, '/ws/Untitled', '/ws/Untitled-2')
    expect(fileOf(useTabsStore.getState().extraGroups[0].tabs[0])).toBe('/ws/Untitled-2')

    useTabsStore.setState({
      extraGroups: [],
      splitCount: 1,
      backgroundWorkspaces: {
        'p:w': { tabs: live, extraGroups: [], splitCount: 1, activeGroupIndex: 0, activeTabId: id },
      },
    })
    useTabsStore.getState().retargetUntitledFile(id, '/ws/Untitled', '/ws/Untitled-3')
    expect(fileOf(useTabsStore.getState().backgroundWorkspaces['p:w'].tabs[0])).toBe('/ws/Untitled-3')
  })
})
