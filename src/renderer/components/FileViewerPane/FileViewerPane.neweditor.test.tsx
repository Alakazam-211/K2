// @vitest-environment jsdom
//
// "New file editor (preview)" (build-editors.md E1b): off by default, the
// file viewer keeps today's toolbar and path bar. On (per computer), markdown
// and code files open in the editor frame on the live shell; every other
// kind keeps today's view.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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
import { setNewEditorPreview } from '@/lib/editor/preview-switch'
import { FileViewerPane } from './FileViewerPane'
import { EditorPreviewSwitch } from '@/components/Editor/EditorPreviewSwitch'
import { CODE_EDITOR_MANIFEST } from '@/components/Settings/sections/CodeEditorSettingsSection'

const FILES: Record<string, string> = {
  '/ws/notes.md': '# Notes\n\nOne two three.\n',
  '/ws/app.ts': 'export const a = 1\n',
  '/ws/rows.csv': 'a,b\n1,2\n',
}

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

describe('the switch gates the editor frame', () => {
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
    setNewEditorPreview(true)
    await open(path)
    const tab = await screen.findByTestId('editor-tab')
    expect(tab.getAttribute('data-kind')).toBe(kind)
    expect(document.querySelector('[title="Refresh"]')).toBeNull()
    expect(document.querySelector('[title="Search (Cmd+F)"]')).toBeNull()
    expect(tab.textContent).not.toContain(path)
    expect(screen.getByText(path.split('/').pop()!).getAttribute('title')).toBe(path)
  })

  it('on: Read shows the file through the buffer', async () => {
    setNewEditorPreview(true)
    await open('/ws/notes.md')
    fireEvent.keyDown(await screen.findByTestId('editor-tab'), { key: 'e', metaKey: true })
    await waitFor(() => expect(screen.getByTestId('editor-read').textContent).toContain('One two three.'))
  })

  it('on: other kinds keep today’s view', async () => {
    setNewEditorPreview(true)
    await open('/ws/rows.csv')
    expect(screen.queryByTestId('editor-tab')).toBeNull()
    expect(document.querySelector('[title="Refresh"]')).toBeTruthy()
  })
})

describe('Settings → Code', () => {
  it('has the preview switch, off by default, per computer', () => {
    expect(CODE_EDITOR_MANIFEST.some((e) => e.id === 'code-editor.new-editor-preview' && e.label === 'New file editor (preview)')).toBe(true)
    renderInPrimaryRoom(<EditorPreviewSwitch />)
    const toggle = screen.getByRole('switch', { name: /New file editor \(preview\)/ })
    expect(toggle.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(toggle)
    expect(window.localStorage.getItem('k2.newFileEditorPreview.v1')).toBe('1')
    expect(toggle.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText(/On this computer only/)).toBeTruthy()
  })
})
