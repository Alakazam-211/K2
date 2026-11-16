// @vitest-environment jsdom
//
// The new editors' switches in a real tab (2026-11-15): a switch re-routes a
// tab that is already open, every markdown extension opens in the markdown
// editor, and a file whose multiplayer core is on opens in the new editor too,
// live (E4a): the kit's Live editor inside it, no "doesn't open live files" line.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({ on: false, core: 'markdown', liveView: true }))
const events = vi.hoisted(() => ({ handlers: new Set<(e: unknown) => void>() }))

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: async (_scope: unknown, route: string) => {
      if (route === 'fs/read-file') return { content: '# Notes\n\nOne two three.\n' }
      if (route === 'docs/cores') return { ok: true, cores: [], file: { core: h.core, on: h.on, liveView: h.liveView } }
      throw new Error(`unexpected route ${route}`)
    },
    daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocCoresChanged: (_scope: unknown, fn: (e: unknown) => void) => {
    events.handlers.add(fn)
    return () => void events.handlers.delete(fn)
  },
}))
vi.mock('@/lib/docs-live-daemon', () => ({
  createDaemonLiveTransport: () => {
    let open = false
    return {
      get open() {
        return open
      },
      async connect() {
        open = true
        return { root: 'source', version: 1, text: '# Notes\n', client: 1, self: 'person:owner', rights: 'edit' }
      },
      send() {},
      onFrame: () => () => undefined,
      onClose: () => () => undefined,
      close() {
        open = false
      },
    }
  },
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { setEditorPreview } from '@/lib/editor/preview-switch'
import { editorFrameRoute } from '@/components/Editor/editorFrameRoute'
import { FileViewerPane } from './FileViewerPane'

// The tab loads the editors lazily: load them once up front, so a busy run doesn't time the first one out.
beforeAll(async () => {
  await import('@/components/Editor/EditorTab')
  await import('@/components/Editor/live/LiveEditorFile')
}, 30_000)

beforeEach(() => {
  h.on = false
  h.core = 'markdown'
  h.liveView = true
})
afterEach(() => {
  cleanup()
  events.handlers.clear()
  window.localStorage.clear()
})

const toolbar = (): Element | null => document.querySelector('[title="Refresh"]')

async function open(path: string): Promise<void> {
  renderInPrimaryRoom(<FileViewerPane filePath={path} paneId="p" tabId="t" />)
  await waitFor(() => expect(document.querySelector('[data-testid="editor-tab"], [title="Refresh"]')).toBeTruthy())
}

describe('the switch and an open tab', () => {
  it('flipping it on re-routes an open markdown tab to the frame, and off brings today’s view back', async () => {
    await open('/ws/notes.md')
    expect(screen.queryByTestId('editor-tab')).toBeNull()
    act(() => setEditorPreview('markdown', true))
    expect((await screen.findByTestId('editor-tab')).getAttribute('data-kind')).toBe('markdown')
    expect(toolbar()).toBeNull()
    act(() => setEditorPreview('markdown', false))
    await waitFor(() => expect(toolbar()).toBeTruthy())
    expect(screen.queryByTestId('editor-tab')).toBeNull()
  })

  it('a change made in another window (the storage event) re-routes it too', async () => {
    await open('/ws/notes.md')
    window.localStorage.setItem('k2.newMarkdownEditor.v1', '1')
    act(() => void window.dispatchEvent(new StorageEvent('storage', { key: 'k2.newMarkdownEditor.v1', newValue: '1' })))
    expect(await screen.findByTestId('editor-tab')).toBeTruthy()
  })
})

describe('every markdown extension opens in the frame when the file is not live', () => {
  it.each(['/ws/notes.md', '/ws/notes.markdown', '/ws/notes.mdx'])('%s', async (path) => {
    setEditorPreview('markdown', true)
    await open(path)
    expect((await screen.findByTestId('editor-tab')).getAttribute('data-kind')).toBe('markdown')
    expect(screen.queryByTestId('editor-preview-live-note')).toBeNull()
  })
})

describe('a live file with the switch on', () => {
  it('opens in the new markdown editor, live, with no line about live files', async () => {
    h.on = true
    setEditorPreview('markdown', true)
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="p" tabId="t" />)
    expect(await screen.findByTestId('ed-live-file')).toBeTruthy()
    expect((await screen.findByTestId('editor-tab')).getAttribute('data-kind')).toBe('markdown')
    expect(screen.getByTestId('editor-tab').hasAttribute('data-live')).toBe(true)
    expect(document.querySelector('[data-testid="file-viewer-live"]')).toBeNull()
    expect(screen.queryByTestId('editor-preview-live-note')).toBeNull()
    expect(toolbar()).toBeNull()
  })

  it('turning the core off keeps the new editor, as a plain file', async () => {
    h.on = true
    setEditorPreview('markdown', true)
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="p" tabId="t" />)
    await screen.findByTestId('ed-live-file')
    h.on = false
    await act(async () => {
      for (const fn of events.handlers) fn({ kind: 'doc_cores_changed', core: 'markdown', on: false, by: 'Anna' })
    })
    await waitFor(() => expect(screen.queryByTestId('ed-live-file')).toBeNull())
    expect(screen.getByTestId('editor-tab').hasAttribute('data-live')).toBe(false)
  })

  it('a live code file opens in the new code editor', async () => {
    h.on = true
    h.core = 'text'
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/app.ts" paneId="p" tabId="t" />)
    await screen.findByTestId('ed-live-file')
    expect(screen.getByTestId('editor-tab').getAttribute('data-kind')).toBe('code')
  })

  it('switch off: a live file keeps today’s live view', async () => {
    h.on = true
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="p" tabId="t" />)
    await waitFor(() => expect(document.querySelector('[data-testid="file-viewer-live"]')).not.toBeNull())
    expect(screen.queryByTestId('ed-live-file')).toBeNull()
  })
})

describe('editorFrameRoute', () => {
  const s = { locked: false, liveOn: false, anyFile: false, deck: false }
  it('routes', () => {
    expect(editorFrameRoute(null, s)).toBe('today')
    expect(editorFrameRoute('markdown', s)).toBe('frame')
    expect(editorFrameRoute('markdown', { ...s, locked: true })).toBe('today')
    expect(editorFrameRoute('markdown', { ...s, liveOn: true })).toBe('live')
    expect(editorFrameRoute('code', { ...s, liveOn: true })).toBe('live')
    expect(editorFrameRoute('code', { ...s, anyFile: true })).toBe('today')
    expect(editorFrameRoute('code', { ...s, deck: true })).toBe('today')
  })
})
