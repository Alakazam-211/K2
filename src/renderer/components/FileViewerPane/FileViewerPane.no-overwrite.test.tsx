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
import { TEXT_LOCK_MESSAGES, isNotFoundReadError, textLockForReadError } from './textLock'

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
    })
    expect(textLockForReadError(refused('File too large (>10MB)', 'too_large'))!.reason).toBe('unreadable')
    expect(textLockForReadError(refused('Is a directory (os error 21)', 'read_failed'))!.reason).toBe('failed')
    expect(textLockForReadError(new Error('File too large (>10MB)'))!.reason).toBe('unreadable')
    expect(textLockForReadError(new Error('boom'))!.reason).toBe('failed')
  })
})
