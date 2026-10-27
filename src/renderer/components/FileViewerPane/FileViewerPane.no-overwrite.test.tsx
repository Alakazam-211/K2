// @vitest-environment jsdom
//
// Data-loss fix (0.46.5): a file `fs/read-file` refuses to return as text
// (.xlsx, .pptx, .tar.gz, .sqlite, Latin-1, over 10 MB) used to open as an
// EMPTY, EDITABLE buffer. One keystroke, then leaving the tab, and the
// autosave-on-leave overwrote the real file with the typed text.
//
// Now only "no such file" (`code: not_found`) opens editable. Every other
// refusal, and a read that returns binary-looking text, locks the pane:
// no editor, and no Save / Cmd+S / save handle / unmount ever writes.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
    daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
  }
})
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
// CodeMirror doesn't type in jsdom: a textarea stands in, wired to the same
// props (code / onChange / onSave / readOnly) the pane passes.
vi.mock('./CodeEditor', () => ({
  getLanguageName: () => 'Plain Text',
  CodeEditor: (props: {
    code: string
    readOnly?: boolean
    onChange?: (c: string) => void
    onSave?: (c: string) => void
  }) => (
    <textarea
      data-testid="fake-editor"
      value={props.code}
      readOnly={props.readOnly}
      onChange={(e) => props.onChange?.(e.target.value)}
    />
  ),
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { useTabsStore } from '@/stores/tabs'
import { FileViewerPane, type FileViewerHandle } from './FileViewerPane'
import {
  REREAD_DELETED_MESSAGE,
  REREAD_FAILED_MESSAGE,
  REREAD_KEPT_MESSAGE,
  TEXT_LOCK_KEPT_MESSAGE,
  TEXT_LOCK_MESSAGES,
  isContentTypeReadError,
  isNotFoundReadError,
  textLockForReadError,
} from './textLock'
import type { Item, PaneGroup, Tab } from '@/stores/tabs'

const refused = (message: string, code: string): DaemonHttpError => new DaemonHttpError(message, 400, code)

function writes(): unknown[][] {
  return daemonCliPost.mock.calls.filter(([, route]) => route === 'fs/write-file')
}

function readFails(err: unknown): void {
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'fs/read-file') throw err
    throw new Error(`unexpected route ${route}`)
  })
}

function readReturns(content: string): void {
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'fs/read-file') return { content }
    throw new Error(`unexpected route ${route}`)
  })
}

function renderPane(filePath: string) {
  const ref = React.createRef<FileViewerHandle | null>()
  const view = renderInPrimaryRoom(
    <FileViewerPane filePath={filePath} paneId="p" paneGroupId="pg" tabId="t" commandRef={ref} />,
  )
  return { ...view, ref }
}

/** Try every way the pane could write: the parent's save handle, Cmd+S,
 *  and the autosave-on-leave unmount. */
async function tryEveryWrite(view: ReturnType<typeof renderPane>): Promise<void> {
  if (!view.ref.current) throw new Error('save handle not bound')
  await act(async () => {
    await view.ref.current!.save()
  })
  const root = document.querySelector('[data-testid="file-viewer-text-lock"]')!.parentElement!
  fireEvent.keyDown(root, { key: 's', metaKey: true })
  view.unmount()
}

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliPost.mockImplementation(async () => ({ success: true }))
})

afterEach(() => {
  cleanup()
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
})

describe('file viewer: a file K2 cannot show as text is never overwritten', () => {
  it.each([
    ['binary', '/ws/budget.xlsx', 'Cannot read binary file'],
    ['binary', '/ws/deck.pptx', 'Cannot read binary file'],
    ['binary', '/ws/app.sqlite', 'Cannot read binary file'],
    ['not_utf8', '/ws/latin1-notes.txt', 'Cannot read binary file'],
    ['too_large', '/ws/huge.log', 'File too large (>10MB)'],
    ['too_large', '/ws/backup.tar.gz', 'File too large (>10MB)'],
    ['binary', '/ws/readme.md', 'Cannot read binary file'],
  ])('code %s (%s): read-only, no editor, no write', async (code, filePath, message) => {
    readFails(refused(message, code))
    const view = renderPane(filePath)

    const lock = await screen.findByTestId('file-viewer-text-lock')
    expect(lock.dataset.reason).toBe('unreadable')
    expect(lock.textContent).toContain(TEXT_LOCK_MESSAGES.unreadable)
    expect(lock.textContent).toContain(message)
    expect(screen.queryByTestId('fake-editor')).toBeNull()
    expect(screen.queryByText('Modified')).toBeNull()

    await tryEveryWrite(view)
    expect(writes()).toEqual([])
  })

  it('a Markdown file stays locked in Edit mode too', async () => {
    readFails(refused('Cannot read binary file', 'not_utf8'))
    renderPane('/ws/old-notes.md')
    await screen.findByTestId('file-viewer-text-lock')
    fireEvent.click(screen.getByText('Edit'))
    expect(screen.getByTestId('file-viewer-text-lock').dataset.reason).toBe('unreadable')
    expect(screen.queryByTestId('fake-editor')).toBeNull()
  })

  it.each([
    ['permission denied', refused('Permission denied', 'permission_denied')],
    ['a network failure', new TypeError('Failed to fetch')],
    ['a 500 with no code', new DaemonHttpError('worker join: panicked', 500, null)],
  ])('%s: locked as a failed read, no write', async (_label, err) => {
    readFails(err)
    const view = renderPane('/ws/config.yaml')

    const lock = await screen.findByTestId('file-viewer-text-lock')
    expect(lock.dataset.reason).toBe('failed')
    expect(lock.textContent).toContain(TEXT_LOCK_MESSAGES.failed)
    expect(screen.queryByTestId('fake-editor')).toBeNull()

    await tryEveryWrite(view)
    expect(writes()).toEqual([])
  })

  it('an older daemon (no code) calling the file binary still locks', async () => {
    readFails(new Error('Cannot read binary file'))
    const view = renderPane('/ws/budget.xlsx')
    expect((await screen.findByTestId('file-viewer-text-lock')).dataset.reason).toBe('unreadable')
    await tryEveryWrite(view)
    expect(writes()).toEqual([])
  })

  it('a read that returns binary-looking text (NUL bytes) locks too', async () => {
    readReturns('PK\u0003\u0004\u0000\u0000rest-of-zip')
    const view = renderPane('/ws/sheet.xlsx')
    const lock = await screen.findByTestId('file-viewer-text-lock')
    expect(lock.dataset.reason).toBe('unreadable')
    expect(screen.queryByTestId('fake-editor')).toBeNull()
    await tryEveryWrite(view)
    expect(writes()).toEqual([])
  })

  it('Refresh lifts the lock once the file reads as text', async () => {
    readFails(refused('Permission denied', 'permission_denied'))
    renderPane('/ws/config.yaml')
    await screen.findByTestId('file-viewer-text-lock')

    readReturns('key: value\n')
    fireEvent.click(screen.getByTitle('Refresh'))
    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    expect(editor.value).toBe('key: value\n')
    expect(screen.queryByTestId('file-viewer-text-lock')).toBeNull()
  })
})

describe('file viewer: a file that does not exist yet still opens as a new file', () => {
  it.each([
    ['code not_found', refused('File not found', 'not_found')],
    ['an older daemon (no code)', new Error('File not found')],
  ])('%s: editable, and leaving the tab saves the typed text', async (_label, err) => {
    readFails(err)
    const view = renderPane('/ws/new-note.txt')

    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    expect(editor.value).toBe('')
    expect(screen.queryByTestId('file-viewer-text-lock')).toBeNull()
    fireEvent.change(editor, { target: { value: 'hello' } })
    await screen.findByText('Modified')

    view.unmount()
    expect(writes()).toHaveLength(1)
    expect(writes()[0][2]).toEqual({ path: '/ws/new-note.txt', content: 'hello' })
  })

  it('the save handle writes a new file', async () => {
    readFails(refused('File not found', 'not_found'))
    const view = renderPane('/ws/draft.md')
    fireEvent.click(await screen.findByText('Edit'))

    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    fireEvent.change(editor, { target: { value: '# Draft\n' } })
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(writes()).toHaveLength(1)
    expect(writes()[0][1]).toBe('fs/write-file')
    expect(writes()[0][2]).toEqual({ path: '/ws/draft.md', content: '# Draft\n' })
    await waitFor(() => expect(screen.queryByText('Modified')).toBeNull())

    // Saved and clean: leaving writes nothing more.
    view.unmount()
    expect(writes()).toHaveLength(1)
  })
})

describe('textLock classification', () => {
  it('only not_found (or an old daemon’s not-found text) is not-found', () => {
    expect(isNotFoundReadError(refused('File not found', 'not_found'))).toBe(true)
    expect(isNotFoundReadError(new Error('File not found'))).toBe(true)
    expect(isNotFoundReadError(new Error('Parent directory does not exist: /ws/x'))).toBe(true)
    // A code wins over the text.
    expect(isNotFoundReadError(refused('File not found', 'binary'))).toBe(false)
    expect(isNotFoundReadError(new Error('Permission denied'))).toBe(false)
    expect(isNotFoundReadError(new TypeError('Failed to fetch'))).toBe(false)
  })

  it('names unreadable vs failed', () => {
    expect(textLockForReadError(refused('File not found', 'not_found'))).toBeNull()
    expect(textLockForReadError(refused('Cannot read binary file', 'not_utf8'))).toEqual({
      reason: 'unreadable',
      detail: 'Cannot read binary file',
      keptText: null,
    })
    expect(textLockForReadError(refused('File too large (>10MB)', 'too_large'))!.reason).toBe('unreadable')
    expect(textLockForReadError(refused('Is a directory (os error 21)', 'read_failed'))!.reason).toBe('failed')
    expect(textLockForReadError(new Error('File too large (>10MB)'))!.reason).toBe('unreadable')
    expect(textLockForReadError(new Error('boom'))!.reason).toBe('failed')
  })
})

// ── After a successful read: a re-read failure must not lose edits ──────
//
// Someone edits a file K2 read as text, then a Refresh fails. A transient
// failure keeps the buffer, the dirty tab and the autosave. A file that
// vanished keeps the edits but is not silently recreated. A file that is
// no longer text locks, keeps the edits copyable, and is never written.

const FILE = '/ws/notes.txt'

function seedTab(filePath: string): void {
  const item: Item = { id: 'p', type: 'file-viewer', data: { filePath }, pinned: false }
  const pg: PaneGroup = { id: 'pg', items: [item], activeItemIndex: 0 }
  const tab: Tab = { id: 't', title: 't', mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
  useTabsStore.setState({ tabs: [tab], activeTabId: 't', splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
}

function tabDirty(): boolean {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === 't')
  if (!tab) throw new Error('tab t missing from the store')
  return tab.isDirty === true
}

/** Read FILE as text, type an edit, then make the next read fail with
 *  `err` (or return `content`) and press Refresh. */
async function editThenRefresh(next: { err: unknown } | { content: string }) {
  seedTab(FILE)
  readReturns('line one\n')
  const view = renderPane(FILE)
  const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
  expect(editor.value).toBe('line one\n')
  fireEvent.change(editor, { target: { value: 'line one\nmy unsaved line\n' } })
  await screen.findByText('Modified')
  await waitFor(() => expect(tabDirty()).toBe(true))

  if ('err' in next) readFails(next.err)
  else readReturns(next.content)
  fireEvent.click(screen.getByTitle('Refresh'))
  await waitFor(() => expect(daemonCliGet.mock.calls.filter(([, r]) => r === 'fs/read-file')).toHaveLength(2))
  await waitFor(() => expect(screen.queryByText('Loading...')).toBeNull())
  return view
}

describe('file viewer: a re-read failure after edits keeps the edits', () => {
  it.each([
    ['permission_denied', refused('Permission denied', 'permission_denied')],
    ['read_failed', refused('Resource busy (os error 16)', 'read_failed')],
    ['a network failure', new TypeError('Failed to fetch')],
    ['a 5xx with no code', new DaemonHttpError('worker join: panicked', 500, null)],
  ])('%s: buffer stays editable and dirty, banner shows, unmount autosaves', async (_label, err) => {
    const view = await editThenRefresh({ err })

    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    expect(editor.value).toBe('line one\nmy unsaved line\n')
    expect(screen.queryByTestId('file-viewer-text-lock')).toBeNull()
    const banner = screen.getByTestId('file-viewer-reread-banner')
    expect(banner.dataset.issue).toBe('transient')
    expect(banner.textContent).toBe(`${REREAD_FAILED_MESSAGE} ${REREAD_KEPT_MESSAGE}`)
    expect(screen.getByText('Modified')).toBeTruthy()
    expect(tabDirty()).toBe(true)

    // Still editable after the failure.
    fireEvent.change(editor, { target: { value: 'line one\nmy unsaved line\nand more\n' } })

    expect(writes()).toEqual([])
    view.unmount()
    expect(writes()).toHaveLength(1)
    expect(writes()[0][2]).toEqual({ path: FILE, content: 'line one\nmy unsaved line\nand more\n' })
  })

  it('a later good read clears the banner and keeps the edits', async () => {
    await editThenRefresh({ err: refused('Permission denied', 'permission_denied') })
    expect(screen.getByTestId('file-viewer-reread-banner')).toBeTruthy()
    readReturns('line one\n')
    fireEvent.click(screen.getByTitle('Refresh'))
    await waitFor(() => expect(screen.queryByTestId('file-viewer-reread-banner')).toBeNull())
    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    expect(editor.value).toBe('line one\nmy unsaved line\n')
  })
})

describe('file viewer: a file deleted or moved after it was read', () => {
  it('keeps the edits and the dirty tab, and leaving does NOT recreate it', async () => {
    const view = await editThenRefresh({ err: refused('File not found', 'not_found') })

    const editor = (await screen.findByTestId('fake-editor')) as HTMLTextAreaElement
    expect(editor.value).toBe('line one\nmy unsaved line\n')
    const banner = screen.getByTestId('file-viewer-reread-banner')
    expect(banner.dataset.issue).toBe('deleted')
    expect(banner.textContent).toBe(
      `${REREAD_FAILED_MESSAGE} ${REREAD_KEPT_MESSAGE} ${REREAD_DELETED_MESSAGE}`,
    )
    expect(tabDirty()).toBe(true)
    expect(screen.queryByTestId('file-viewer-text-lock')).toBeNull()

    view.unmount()
    expect(writes()).toEqual([])
  })

  it('an older daemon’s plain "File not found" is treated the same', async () => {
    const view = await editThenRefresh({ err: new Error('File not found') })
    expect(screen.getByTestId('file-viewer-reread-banner').dataset.issue).toBe('deleted')
    view.unmount()
    expect(writes()).toEqual([])
  })

  it('Save in the leave guard writes it back on unmount', async () => {
    const view = await editThenRefresh({ err: refused('File not found', 'not_found') })
    useTabsStore.getState().markTabsSaveConfirmed(['t'])
    view.unmount()
    expect(writes()).toHaveLength(1)
    expect(writes()[0][2]).toEqual({ path: FILE, content: 'line one\nmy unsaved line\n' })
  })

  it('an explicit save writes it back and clears the banner', async () => {
    const view = await editThenRefresh({ err: refused('File not found', 'not_found') })
    await act(async () => {
      await view.ref.current!.save()
    })
    expect(writes()).toHaveLength(1)
    expect(writes()[0][2]).toEqual({ path: FILE, content: 'line one\nmy unsaved line\n' })
    await waitFor(() => expect(screen.queryByTestId('file-viewer-reread-banner')).toBeNull())
    await waitFor(() => expect(tabDirty()).toBe(false))
  })

  it('Discard in the leave guard still drops the edit, and both flags drain', async () => {
    const view = await editThenRefresh({ err: refused('File not found', 'not_found') })
    useTabsStore.getState().markTabsDiscardPending(['t'])
    useTabsStore.getState().markTabsSaveConfirmed(['t'])
    view.unmount()
    expect(writes()).toEqual([])
    expect(useTabsStore.getState().consumeDiscardPending('t')).toBe(false)
    expect(useTabsStore.getState().consumeSaveConfirmed('t')).toBe(false)
  })
})

describe('file viewer: the file turned non-text after edits', () => {
  it.each([
    ['code binary', { err: refused('Cannot read binary file', 'binary') }],
    ['code not_utf8', { err: refused('Cannot read binary file', 'not_utf8') }],
    ['code too_large', { err: refused('File too large (>10MB)', 'too_large') }],
    ['NUL bytes in a good read', { content: 'PK\u0003\u0004\u0000\u0000zip' }],
  ] as const)('%s: locks, keeps the text copyable, never writes', async (_label, next) => {
    const view = await editThenRefresh(next)

    const lock = await screen.findByTestId('file-viewer-text-lock')
    expect(lock.dataset.reason).toBe('unreadable')
    expect(lock.textContent).toContain(TEXT_LOCK_KEPT_MESSAGE)
    expect(screen.queryByTestId('fake-editor')).toBeNull()
    expect(screen.queryByTestId('file-viewer-reread-banner')).toBeNull()

    const kept = screen.getByTestId('file-viewer-kept-text') as HTMLTextAreaElement
    expect(kept.value).toBe('line one\nmy unsaved line\n')
    expect(kept.readOnly).toBe(true)
    expect(kept.disabled).toBe(false)
    // Selectable for copying.
    kept.focus()
    kept.setSelectionRange(0, kept.value.length)
    expect(kept.value.slice(kept.selectionStart!, kept.selectionEnd!)).toBe('line one\nmy unsaved line\n')
    // Typing into it changes nothing.
    fireEvent.change(kept, { target: { value: 'typed over' } })
    expect((screen.getByTestId('file-viewer-kept-text') as HTMLTextAreaElement).value).toBe(
      'line one\nmy unsaved line\n',
    )

    expect(tabDirty()).toBe(true)

    await tryEveryWrite(view)
    expect(writes()).toEqual([])
  })

  it('even Save in the leave guard does not write a file that is no longer text', async () => {
    const view = await editThenRefresh({ err: refused('Cannot read binary file', 'binary') })
    useTabsStore.getState().markTabsSaveConfirmed(['t'])
    view.unmount()
    expect(writes()).toEqual([])
  })
})

describe('textLock: content-type refusals', () => {
  it('binary, not_utf8, too_large (and old-daemon text) are content-type; others are not', () => {
    for (const code of ['binary', 'not_utf8', 'too_large']) {
      expect(isContentTypeReadError(refused('x', code))).toBe(true)
    }
    for (const code of ['not_found', 'permission_denied', 'read_failed', 'invalid_path']) {
      expect(isContentTypeReadError(refused('Cannot read binary file', code))).toBe(false)
    }
    expect(isContentTypeReadError(new Error('Cannot read binary file'))).toBe(true)
    expect(isContentTypeReadError(new Error('File too large (>10MB)'))).toBe(true)
    expect(isContentTypeReadError(new TypeError('Failed to fetch'))).toBe(false)
  })
})
