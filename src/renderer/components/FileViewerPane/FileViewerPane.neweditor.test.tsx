// @vitest-environment jsdom
//
// The new editors (build-editors.md E1b): "New markdown editor" and "New code
// editor", each off by default, and the file viewer keeps today's toolbar and
// path bar. On (per computer), each opens its own files on the shared frame;
// every other kind keeps today's view.

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

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
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { setEditorPreview } from '@/lib/editor/preview-switch'
import { FileViewerPane } from './FileViewerPane'
import { EDITOR_PREVIEW_MANIFEST, EditorPreviewSwitch } from '@/components/Editor/EditorPreviewSwitch'
import { CODE_EDITOR_MANIFEST } from '@/components/Settings/sections/CodeEditorSettingsSection'

const FILES: Record<string, string> = {
  '/ws/notes.md': '# Notes\n\nOne two three.\n',
  '/ws/app.ts': 'export const a = 1\n',
  '/ws/rows.csv': 'a,b\n1,2\n',
}

// The tab loads the editors lazily: load them once up front, so a busy run doesn't time the first one out.
beforeAll(async () => {
  await import('@/components/Editor/EditorTab')
}, 30_000)

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string, q: { path: string }) => {
    if (route === 'fs/read-file') return { content: FILES[q.path] }
    throw new Error(`unexpected route ${route}`)
  })
})

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

async function open(path: string): Promise<void> {
  renderInPrimaryRoom(<FileViewerPane filePath={path} paneId="p" tabId="t" />)
  await waitFor(() => expect(document.querySelector('[data-testid="editor-tab"], [title="Refresh"]')).toBeTruthy())
}

const setBoth = (): void => {
  setEditorPreview('markdown', true)
  setEditorPreview('code', true)
}

describe('the switches gate the new editors', () => {
  it('off (the default): markdown keeps today’s toolbar and path bar', async () => {
    await open('/ws/notes.md')
    expect(screen.queryByTestId('editor-tab')).toBeNull()
    expect(document.querySelector('[title="Refresh"]')).toBeTruthy()
    expect(document.querySelector('span[title="/ws/notes.md"]')?.textContent).toBe('/ws/notes.md')
  })

  it.each([
    ['markdown', '/ws/notes.md', 'markdown'],
    ['code', '/ws/app.ts', 'code'],
  ])('on: %s opens in the editor frame, without the toolbar or the path bar', async (_k, path, kind) => {
    setBoth()
    await open(path)
    const tab = await screen.findByTestId('editor-tab')
    expect(tab.getAttribute('data-kind')).toBe(kind)
    expect(document.querySelector('[title="Refresh"]')).toBeNull()
    expect(document.querySelector('[title="Search (Cmd+F)"]')).toBeNull()
    expect(tab.textContent).not.toContain(path)
    expect(screen.getByText(path.split('/').pop()!).getAttribute('title')).toBe(path)
  })

  it('on: Read shows the file through the buffer', async () => {
    setBoth()
    await open('/ws/notes.md')
    fireEvent.keyDown(await screen.findByTestId('editor-tab'), { key: 'e', metaKey: true })
    await waitFor(() => expect(screen.getByTestId('editor-read').textContent).toContain('One two three.'))
  })

  it('each switch opens only its own files', async () => {
    setEditorPreview('markdown', true)
    await open('/ws/app.ts')
    expect(screen.queryByTestId('editor-tab')).toBeNull()
    cleanup()
    await open('/ws/notes.md')
    expect((await screen.findByTestId('editor-tab')).getAttribute('data-kind')).toBe('markdown')
    cleanup()
    setEditorPreview('markdown', false)
    setEditorPreview('code', true)
    await open('/ws/notes.md')
    expect(screen.queryByTestId('editor-tab')).toBeNull()
    cleanup()
    await open('/ws/rows.csv')
    expect((await screen.findByTestId('editor-tab')).getAttribute('data-kind')).toBe('code')
  })

  it('code: a CSV opens in its table as Preview, with Source beside it', async () => {
    setEditorPreview('code', true)
    await open('/ws/rows.csv')
    const tab = await screen.findByTestId('editor-tab')
    expect(tab.getAttribute('data-mode')).toBe('preview')
    expect(screen.getByTestId('editor-preview')).toBeTruthy()
    fireEvent.click(screen.getByRole('radio', { name: 'Source' }))
    await waitFor(() => expect(tab.getAttribute('data-mode')).toBe('source'))
    expect(screen.queryByTestId('editor-preview')).toBeNull()
  })

  it('code: plain code has no mode control', async () => {
    setEditorPreview('code', true)
    await open('/ws/app.ts')
    expect((await screen.findByTestId('editor-tab')).getAttribute('data-mode')).toBe('source')
    expect(screen.queryByRole('radio', { name: 'Source' })).toBeNull()
    expect(screen.queryByRole('radio', { name: 'Write' })).toBeNull()
  })
})

describe('Settings → Multiplayer', () => {
  it('has the two switches, off by default, per computer; Code has none', () => {
    expect(CODE_EDITOR_MANIFEST.some((e) => /new editor|new-editor/.test(e.id))).toBe(false)
    expect(EDITOR_PREVIEW_MANIFEST.map((e) => e.label)).toEqual(['New markdown editor', 'New code editor'])
    renderInPrimaryRoom(<EditorPreviewSwitch />)
    const md = screen.getByRole('switch', { name: /New markdown editor/ })
    const code = screen.getByRole('switch', { name: /New code editor/ })
    expect(md.getAttribute('aria-checked')).toBe('false')
    expect(code.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(md)
    expect(window.localStorage.getItem('k2.newMarkdownEditor.v1')).toBe('1')
    expect(window.localStorage.getItem('k2.newCodeEditor.v1')).toBeNull()
    expect(md.getAttribute('aria-checked')).toBe('true')
    expect(code.getAttribute('aria-checked')).toBe('false')
    expect(screen.getAllByText(/On this computer only/)).toHaveLength(2)
  })
})
