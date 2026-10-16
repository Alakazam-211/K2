// @vitest-environment jsdom
//
// The file viewer's toolbar pin is the tab strip's pin (Rosson 2026-10-16):
// for any file kind it toggles THIS viewer's tab's pin flag in place (no
// second tab), and it is hidden when no pinnable tab holds the viewer.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
    daemonCliPost: vi.fn(async () => {
      throw new Error('unexpected POST')
    }),
  }
})
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'
import { FileViewerPane } from './FileViewerPane'

function fileTab(id: string, filePath: string): Tab {
  const item: Item = { id: `${id}-item`, type: 'file-viewer', data: { filePath }, pinned: false }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: id, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
}

async function pinButton(): Promise<HTMLElement> {
  let el: HTMLElement | null = null
  await waitFor(() => {
    el = document.querySelector('[data-file-viewer-pin]')
    if (!el) throw new Error('viewer pin not rendered')
  })
  return el as unknown as HTMLElement
}

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'fs/read-file') return { content: '# Notes\n' }
    throw new Error(`unexpected route ${route}`)
  })
})

afterEach(() => {
  cleanup()
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
})

describe('file viewer pin', () => {
  it.each([
    ['Markdown', '/ws/notes.md'],
    ['HTML', '/ws/page.html'],
  ])('%s: toggles the pin of the tab that holds the viewer', async (_kind, filePath) => {
    useTabsStore.setState({
      tabs: [fileTab('other', '/ws/other.txt'), fileTab('v', filePath)],
      activeTabId: 'v',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })
    renderInPrimaryRoom(<FileViewerPane filePath={filePath} paneId="v-item" paneGroupId="v-pg" tabId="v" />)

    const pin = await pinButton()
    expect(pin.dataset.fileViewerPin).toBe('unpinned')
    expect(pin.getAttribute('title')).toBe('Pin tab (keeps it at the left)')
    fireEvent.click(pin)

    const tabs = useTabsStore.getState().tabs
    expect(tabs.map((t) => t.id)).toEqual(['v', 'other'])
    expect(tabs[0].isPinnedFile).toBe(true)
    await waitFor(() => expect(document.querySelector('[data-file-viewer-pin]')?.getAttribute('data-file-viewer-pin')).toBe('pinned'))

    fireEvent.click(await pinButton())
    expect(useTabsStore.getState().tabs.find((t) => t.id === 'v')!.isPinnedFile).toBeUndefined()
  })

  it('is hidden when no tab holds the viewer', async () => {
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="x" tabId="missing" />)
    await waitFor(() => expect(document.body.textContent).toContain('notes.md'))
    expect(document.querySelector('[data-file-viewer-pin]')).toBeNull()
  })
})
