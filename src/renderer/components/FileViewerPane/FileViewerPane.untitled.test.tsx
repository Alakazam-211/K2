// @vitest-environment jsdom
//
// A new document's first write never overwrites: Save, the save handle and
// autosave-on-leave all go through `fs/create-untitled`, and the tab follows the
// name it got. Once the file exists, saves are ordinary writes again.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const pollOpts = vi.hoisted(() => ({ current: null as null | { shouldSkip?: () => boolean } }))
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
    daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
  }
})
vi.mock('@/lib/host-file-text-poll', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/host-file-text-poll')>()
  return {
    ...actual,
    startHostFileTextPoll: (opts: { shouldSkip?: () => boolean }) => {
      pollOpts.current = opts
      return () => undefined
    },
  }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('./CodeEditor', () => ({
  getLanguageName: () => 'Plain Text',
  CodeEditor: (props: { code: string; onChange?: (c: string) => void }) => (
    <textarea
      data-testid="fake-editor"
      value={props.code}
      onChange={(e) => props.onChange?.(e.target.value)}
    />
  ),
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { useTabsStore, type FileViewerItemData, type Tab } from '@/stores/tabs'
import { FileViewerPane, type FileViewerHandle } from './FileViewerPane'

const notFound = new DaemonHttpError('File not found', 400, 'not_found')

function calls(route: string): unknown[][] {
  return daemonCliPost.mock.calls.filter(([, r]) => r === route)
}

/** The tab store holds the tab; the pane follows its `filePath`, as PaneGroupView does. */
function seedTab(filePath: string, others: Tab[] = []): void {
  const tab: Tab = {
    id: 't',
    title: filePath.split('/').pop()!,
    mosaicTree: 'pg',
    paneGroups: new Map([
      ['pg', { id: 'pg', activeItemIndex: 0, items: [{ id: 'i', type: 'file-viewer', data: { filePath }, pinned: true }] }],
    ]),
  }
  useTabsStore.setState({ tabs: [tab, ...others], activeTabId: 't' })
}

function tabFile(): string {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === 't')!
  return (tab.paneGroups.get('pg')!.items[0].data as FileViewerItemData).filePath
}

function renderPane(filePath: string) {
  const ref = React.createRef<FileViewerHandle | null>()
  const view = renderInPrimaryRoom(
    <FileViewerPane filePath={filePath} paneId="pg" paneGroupId="pg" tabId="t" commandRef={ref} />,
  )
  return { ...view, ref }
}

async function typeIntoNewDocument(text: string): Promise<void> {
  const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
  fireEvent.change(editor, { target: { value: text } })
  await screen.findByText('Modified')
}

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  pollOpts.current = null
  daemonCliGet.mockImplementation(async () => {
    throw notFound
  })
  daemonCliPost.mockImplementation(async (_s: unknown, route: string, body: { prefer?: string }) =>
    route === 'fs/create-untitled' ? { name: body.prefer } : { success: true },
  )
})

afterEach(() => {
  cleanup()
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
})

describe('a new document’s first write', () => {
  it('Save creates the file, never writing over one', async () => {
    seedTab('/ws/Untitled')
    const view = renderPane('/ws/Untitled')
    await typeIntoNewDocument('hello')
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(calls('fs/write-file')).toEqual([])
    expect(calls('fs/create-untitled').map((c) => c[2])).toEqual([
      { dir: '/ws', prefer: 'Untitled', taken: [], content: 'hello' },
    ])
    await waitFor(() => expect(screen.queryByText('Modified')).toBeNull())
    expect(tabFile()).toBe('/ws/Untitled')
  })

  it('a name someone took moves the tab, and the text is in the new file', async () => {
    seedTab('/ws/Untitled')
    daemonCliPost.mockImplementation(async () => ({ name: 'Untitled-2' }))
    const view = renderPane('/ws/Untitled')
    await typeIntoNewDocument('mine')
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(tabFile()).toBe('/ws/Untitled-2')
    expect(useTabsStore.getState().tabs[0].title).toBe('Untitled-2')
    expect(calls('fs/write-file')).toEqual([])
  })

  it('the next save writes to the file the first one made', async () => {
    seedTab('/ws/Untitled')
    const view = renderPane('/ws/Untitled')
    await typeIntoNewDocument('one')
    await act(async () => {
      await view.ref.current!.save()
    })
    await waitFor(() => expect(screen.queryByText('Modified')).toBeNull())
    await typeIntoNewDocument('two')
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(calls('fs/create-untitled')).toHaveLength(1)
    expect(calls('fs/write-file').map((c) => c[2])).toEqual([{ path: '/ws/Untitled', content: 'two' }])
  })

  it('two saves at once make one file', async () => {
    seedTab('/ws/Untitled')
    let finishCreate: (v: { name: string }) => void = () => undefined
    daemonCliPost.mockImplementation((_s: unknown, route: string) =>
      route === 'fs/create-untitled'
        ? new Promise((resolve) => {
            finishCreate = resolve
          })
        : Promise.resolve({ success: true }),
    )
    const view = renderPane('/ws/Untitled')
    await typeIntoNewDocument('both')
    let first: Promise<void> = Promise.resolve()
    let second: Promise<void> = Promise.resolve()
    await act(async () => {
      first = view.ref.current!.save()
      second = view.ref.current!.save()
    })
    await act(async () => {
      finishCreate({ name: 'Untitled' })
      await first
      await second
    })
    expect(calls('fs/create-untitled')).toHaveLength(1)
  })

  it('leaving the tab creates the file too', async () => {
    seedTab('/ws/Untitled-3.md')
    const view = renderPane('/ws/Untitled-3.md')
    fireEvent.click(await screen.findByText('Edit'))
    await typeIntoNewDocument('left')
    view.unmount()
    expect(calls('fs/write-file')).toEqual([])
    expect(calls('fs/create-untitled').map((c) => c[2])).toEqual([
      { dir: '/ws', prefer: 'Untitled-3.md', taken: [], content: 'left' },
    ])
  })

  it('tells the daemon which names other tabs hold', async () => {
    const other: Tab = {
      id: 'o',
      title: 'Untitled',
      mosaicTree: 'x',
      paneGroups: new Map([
        ['x', { id: 'x', activeItemIndex: 0, items: [{ id: 'y', type: 'file-viewer', data: { filePath: '/ws/Untitled' }, pinned: true }] }],
      ]),
    }
    seedTab('/ws/Untitled-2', [other])
    const view = renderPane('/ws/Untitled-2')
    await typeIntoNewDocument('x')
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(calls('fs/create-untitled')[0][2]).toMatchObject({ prefer: 'Untitled-2', taken: ['Untitled'] })
  })

  it('a refused create keeps the text and stays unsaved', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    seedTab('/ws/Untitled')
    daemonCliPost.mockRejectedValue(new DaemonHttpError('Permission denied', 400, null))
    const view = renderPane('/ws/Untitled')
    await typeIntoNewDocument('keep me')
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(screen.getByText('Modified')).toBeTruthy()
    expect((screen.getByTestId('fake-editor') as HTMLTextAreaElement).value).toBe('keep me')
    expect(calls('fs/write-file')).toEqual([])
    error.mockRestore()
  })
})

describe('a file that exists stays an ordinary file', () => {
  it('an Untitled file K2 read saves with a plain write', async () => {
    daemonCliGet.mockImplementation(async () => ({ content: 'on disk' }))
    seedTab('/ws/Untitled')
    const view = renderPane('/ws/Untitled')
    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    await waitFor(() => expect(editor.value).toBe('on disk'))
    fireEvent.change(editor, { target: { value: 'edited' } })
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(calls('fs/create-untitled')).toEqual([])
    expect(calls('fs/write-file').map((c) => c[2])).toEqual([{ path: '/ws/Untitled', content: 'edited' }])
  })

  it('a missing file with any other name saves as it always did', async () => {
    seedTab('/ws/notes.txt')
    const view = renderPane('/ws/notes.txt')
    await typeIntoNewDocument('x')
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(calls('fs/create-untitled')).toEqual([])
    expect(calls('fs/write-file')).toHaveLength(1)
  })
})

describe('the poll', () => {
  it('leaves an empty new document alone, and watches it once it is a file', async () => {
    seedTab('/ws/Untitled')
    const view = renderPane('/ws/Untitled')
    await screen.findByTestId('fake-editor')
    await waitFor(() => expect(pollOpts.current).not.toBeNull())
    expect(pollOpts.current!.shouldSkip!()).toBe(true)

    await typeIntoNewDocument('x')
    await act(async () => {
      await view.ref.current!.save()
    })
    await waitFor(() => expect(screen.queryByText('Modified')).toBeNull())
    expect(pollOpts.current!.shouldSkip!()).toBe(false)
  })
})
