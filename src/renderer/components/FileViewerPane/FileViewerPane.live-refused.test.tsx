// @vitest-environment jsdom
//
// A live open the daemon refuses never blanks the page (Rosson 2026-11-17: a
// 10 KB HTML file in another workspace's `.k2/agent/work/` opened to "Live
// editing stopped. This file can't be edited live: too large, or not text." and
// an empty preview). The daemon refuses it as `protected` (the workspace's
// private .k2 folder); the tab then:
//   - draws the page from its plain read, never from the live text it never got;
//   - says the real reason, in the daemon's words, never "too large, or not text";
//   - opens the usual way (the plain editor), since that file is never live.
// And the "undefined" tile: a read whose body isn't `{content}` is an error,
// never the word "undefined" drawn as the page.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, screen, waitFor } from '@testing-library/react'

const PAGE = '<!doctype html>\n<html><head><style>body { font: 14px sans-serif; }</style></head><body><h1>k5 CLI progress</h1><p>Slice 10 landed.</p></body></html>\n'
const WHY = ".k2/agent/work/k5-cli-progress.html is in the workspace's private .k2 folder, where only Markdown and files under .k2/wiki, .k2/prds, .k2/docs or .k2/conflicts are live docs"
const h = vi.hoisted(() => ({ read: null as unknown, refuse: null as Record<string, unknown> | null, connects: 0 }))

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: async (_scope: unknown, route: string) => {
      if (route === 'fs/read-file') return h.read
      if (route === 'docs/cores') return { ok: true, cores: [], file: { core: 'text', on: true, liveView: true } }
      throw new Error(`unexpected route ${route}`)
    },
    daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocCoresChanged: () => () => undefined,
}))
vi.mock('@/lib/docs-live-daemon', async () => {
  const { LiveConnectError, parseClose } = await import('@/lib/docs-live-transport')
  return {
    createDaemonLiveTransport: () => ({
      open: false,
      async connect() {
        h.connects += 1
        // The daemon's `closed` frame before any welcome: the refusal.
        if (h.refuse) throw new LiveConnectError(parseClose(h.refuse))
        throw new Error('this test only refuses')
      },
      send() {},
      onFrame: () => () => undefined,
      onClose: () => () => undefined,
      close() {},
    }),
  }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { setEditorPreview } from '@/lib/editor/preview-switch'
import { NOT_FILE_TEXT, fileTextOf } from '@/lib/read-file-text'
import { FileViewerPane } from './FileViewerPane'

const FILE = '/home/me/project/.k2/agent/work/progress.html'

beforeAll(async () => {
  await import('@/components/Editor/EditorTab')
  await import('@/components/Editor/live/LiveEditorFile')
}, 30_000)

beforeEach(() => {
  h.read = { content: PAGE, path: FILE, name: 'k5-cli-progress.html' }
  h.refuse = { type: 'closed', reason: 'declined', retry: false, code: 'protected', message: WHY }
  h.connects = 0
})
afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const pageFrame = (): HTMLIFrameElement | null => document.querySelector('iframe[srcdoc]')

describe('a refused live open of an HTML page', () => {
  it('draws the page from its plain read, names the .k2 folder, and opens the usual way', async () => {
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    const bar = await screen.findByTestId('file-viewer-live-refused', {}, { timeout: 5000 })
    expect(bar.textContent).toBe(`This file can’t be edited live: ${WHY}. It opens the usual way.`)
    expect(h.connects).toBe(1)
    // Handed over: the plain editor, not the live one (no second open, no closed notice).
    await waitFor(() => expect(screen.queryByTestId('ed-live-file')).toBeNull())
    expect(screen.getByTestId('editor-tab').hasAttribute('data-live')).toBe(false)
    // The preview is the page as read, never blank.
    await waitFor(() => expect(pageFrame()?.getAttribute('srcdoc') ?? '').toContain('<h1>k5 CLI progress</h1>'))
    expect(document.body.textContent).not.toContain('too large, or not text')
    expect(h.connects).toBe(1)
  })

  it('a refusal with no words of its own still draws the page', async () => {
    h.refuse = { type: 'closed', reason: 'declined', retry: false }
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    expect((await screen.findByTestId('file-viewer-live-refused', {}, { timeout: 5000 })).textContent).toBe('This file can’t be edited live: too large, or not text. It opens the usual way.')
    await waitFor(() => expect(pageFrame()?.getAttribute('srcdoc') ?? '').toContain('<h1>k5 CLI progress</h1>'))
  })
})

describe('a read that is not the file’s text', () => {
  it('is an error, never "undefined" drawn as the page', async () => {
    // A 200 whose body isn't JSON (a proxy's page) parses to a string: no `content`.
    h.read = '<html>Sign in</html>'
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/report.html" paneId="p" tabId="t" />)
    await waitFor(() => expect(document.body.textContent).toContain(NOT_FILE_TEXT))
    expect(pageFrame()?.getAttribute('srcdoc') ?? '').not.toContain('undefined')
    expect(() => fileTextOf(undefined)).toThrow(NOT_FILE_TEXT)
    expect(() => fileTextOf({ path: '/ws/report.html' })).toThrow(NOT_FILE_TEXT)
    expect(fileTextOf({ content: '' })).toBe('')
  })
})
