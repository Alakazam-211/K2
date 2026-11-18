// @vitest-environment jsdom
//
// A live open the daemon refuses never blanks the page (Rosson 2026-11-17: a
// 10 KB HTML file in another workspace's `.k2/agent/work/` opened to "Live
// editing stopped. This file can't be edited live: too large, or not text." and
// an empty preview: a 0.46.13 app over a daemon that refused it as
// `protected`). Since the 11-18 ruling such a work page opens live; the vault
// is still `protected`, and is the refusal here. The tab then:
//   - draws the page from its plain read, never from the live text it never got;
//   - says the real reason, in the daemon's words, never "too large, or not text";
//   - opens the usual way (the plain editor), since that file is never live;
//   - offers Reload, which reads it again and retries the live open.
// The same holds for a session that closes `declined` after its welcome, and
// for an older server's close with only a code (mapped to plain words) or
// only the kit's report; with no words at all, "Live editing isn’t available
// for this file here."
// And the "undefined" tile: a read whose body isn't `{content}` is an error,
// never the word "undefined" drawn as the page.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, renderHook, screen, waitFor } from '@testing-library/react'

const PAGE = '<!doctype html>\n<html><head><style>body { font: 14px sans-serif; }</style></head><body><h1>k5 CLI progress</h1><p>Slice 10 landed.</p></body></html>\n'
const WHY = '.k2/vault/progress.html is in the vault folder; it is never a live doc'
const h = vi.hoisted(() => ({
  read: null as unknown,
  refuse: null as Record<string, unknown> | null,
  // A session that opens (its welcome) and then closes with this frame.
  closeAfterWelcome: null as Record<string, unknown> | null,
  // The welcome's text (an older server's that came through empty, or the page).
  welcomeText: '',
  end: null as ((e: unknown) => void) | null,
  connects: 0,
}))

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
    createDaemonLiveTransport: () => {
      const closeCbs = new Set<(e: unknown) => void>()
      return {
        open: false,
        async connect() {
          h.connects += 1
          // The daemon's `closed` frame before any welcome: the refusal.
          if (h.refuse) throw new LiveConnectError(parseClose(h.refuse))
          if (h.closeAfterWelcome) {
            const frame = h.closeAfterWelcome
            h.end = () => {
              for (const cb of [...closeCbs]) cb(parseClose(frame))
            }
            // An older server's welcome whose live text came through empty.
            return { root: 'source', version: 1, text: h.welcomeText, client: 7, self: 'person:owner', rights: 'edit' }
          }
          throw new Error('this test only refuses')
        },
        send() {},
        onFrame: () => () => undefined,
        onClose: (cb: (e: unknown) => void) => {
          closeCbs.add(cb)
          return () => void closeCbs.delete(cb)
        },
        close() {},
      }
    },
  }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { setEditorPreview } from '@/lib/editor/preview-switch'
import { NOT_FILE_TEXT, fileTextOf } from '@/lib/read-file-text'
import { FileViewerPane } from './FileViewerPane'
import { useLiveRefused } from './live-refused'

const FILE = '/home/me/project/.k2/vault/progress.html'

beforeAll(async () => {
  await import('@/components/Editor/EditorTab')
  await import('@/components/Editor/live/LiveEditorFile')
}, 30_000)

beforeEach(() => {
  h.read = { content: PAGE, path: FILE, name: 'k5-cli-progress.html' }
  h.refuse = { type: 'closed', reason: 'declined', retry: false, code: 'protected', message: WHY }
  h.closeAfterWelcome = null
  h.welcomeText = ''
  h.end = null
  h.connects = 0
})
afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const pageFrame = (): HTMLIFrameElement | null => document.querySelector('iframe[srcdoc]')

describe('a refused live open of an HTML page', () => {
  it('draws the page from its plain read, names the vault, and opens the usual way', async () => {
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    const bar = await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })
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
    expect((await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })).textContent).toBe('Live editing isn’t available for this file here. It opens the usual way.')
    await waitFor(() => expect(pageFrame()?.getAttribute('srcdoc') ?? '').toContain('<h1>k5 CLI progress</h1>'))
    expect(document.body.textContent).not.toContain('too large, or not text')
  })

  it('an older server’s refusal with only a code says it in plain words: another workspace opens read only', async () => {
    h.refuse = { type: 'closed', reason: 'declined', retry: false, code: 'path_escapes' }
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    expect((await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })).textContent).toBe('This file is in another workspace’s folder, so it opens read-only here. Open it from that workspace to edit it live.')
    await waitFor(() => expect(pageFrame()?.getAttribute('srcdoc') ?? '').toContain('<h1>k5 CLI progress</h1>'))
    // The words say read only, and it is: the plain editor gets the reason.
    const { result } = renderHook(() => useLiveRefused(FILE))
    act(() => result.current.onLiveRefused({ reason: 'declined', code: 'path_escapes' }))
    expect(result.current.refusedReadOnly).toBe('This file is in another workspace’s folder, so it opens read-only here. Open it from that workspace to edit it live.')
    act(() => result.current.onLiveRefused({ reason: 'declined', code: 'protected', message: WHY }))
    expect(result.current.refusedReadOnly).toBeNull()
  })

  it('the kit’s own report, when the daemon added no words, is what the bar says', async () => {
    h.refuse = { type: 'closed', reason: 'declined', retry: false, error: { id: 'MPC-E000', message: 'this doc has no `source` text' } }
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    expect((await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })).textContent).toBe('This file can’t be edited live: this doc has no `source` text. It opens the usual way.')
  })
})

describe('a live session that closes declined after it opened', () => {
  it('keeps the page (the plain read, never the empty live text) and says the frame’s own words', async () => {
    h.refuse = null
    h.closeAfterWelcome = { type: 'closed', reason: 'declined', retry: false, code: 'some_new_rule', message: 'the doc moved to another engine' }
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    await waitFor(() => expect(screen.getByTestId('ed-live-file').dataset.liveState).toBe('live'), { timeout: 5000 })
    act(() => h.end?.(null))
    const bar = await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })
    expect(bar.textContent).toBe('This file can’t be edited live: the doc moved to another engine. It opens the usual way.')
    await waitFor(() => expect(screen.queryByTestId('ed-live-file')).toBeNull())
    await waitFor(() => expect(pageFrame()?.getAttribute('srcdoc') ?? '').toContain('<h1>k5 CLI progress</h1>'))
    expect(document.body.textContent).not.toContain('too large, or not text')
    expect(h.connects).toBe(1)
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

describe('Reload on the refused bar', () => {
  it('reads the file again and re-opens a file that is live now (healed), in the same tab', async () => {
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })
    expect(h.connects).toBe(1)
    // The daemon takes it now.
    h.refuse = null
    h.closeAfterWelcome = { type: 'closed', reason: 'declined', retry: false }
    h.welcomeText = PAGE
    h.read = { content: PAGE.replace('Slice 10 landed.', 'Slice 11 landed.'), path: FILE, name: 'progress.html' }
    fireEvent.click(screen.getByTestId('file-viewer-live-reload'))
    await waitFor(() => expect(screen.getByTestId('ed-live-file').dataset.liveState).toBe('live'), { timeout: 5000 })
    expect(h.connects).toBe(2)
    expect(screen.queryByTestId('file-viewer-live-refused')).toBeNull()
  })

  it('a file still refused stays on its preview from the read, with the reason, never blank', async () => {
    setEditorPreview('code', true)
    renderInPrimaryRoom(<FileViewerPane filePath={FILE} paneId="p" tabId="t" />)
    await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })
    h.read = { content: PAGE.replace('Slice 10 landed.', 'Slice 11 landed.'), path: FILE, name: 'progress.html' }
    fireEvent.click(screen.getByTestId('file-viewer-live-reload'))
    await waitFor(() => expect(h.connects).toBe(2), { timeout: 5000 })
    const bar = await screen.findByTestId('file-viewer-live-refused-words', {}, { timeout: 5000 })
    expect(bar.textContent).toContain(`This file can’t be edited live: ${WHY}. It opens the usual way.`)
    // The page as read again (the new read), never blank.
    await waitFor(() => expect(pageFrame()?.getAttribute('srcdoc') ?? '').toContain('Slice 11 landed.'))
    expect(screen.queryByTestId('ed-live-file')).toBeNull()
  })
})
