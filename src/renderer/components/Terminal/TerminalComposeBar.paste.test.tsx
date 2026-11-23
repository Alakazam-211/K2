// @vitest-environment jsdom
// Cmd+V with a picture or a file on the clipboard, in the agent Thread's
// message box (0.46.18; 0.46.17 had no paste handler there, so the field's
// own paste dropped the image without a word). Fail loud.
//
//   1. An image item (Chromium: `items` only, no name): uploaded through
//      the Thread's attach path (`executeBrowserFileDrop`, as a pick of a
//      browser file), a chip under its own name, sent as `attachments`.
//   2. A file item (WKWebView: `files`, a named file): a chip by its name.
//   3. Text only: left to the field (default not prevented), no upload.
//   4. An image plus text: the chip, and the text still pastes; plus
//      Finder-style text (only the file's name): the name doesn't paste.
//   5. A box without Thread files (the PTY bar): the uploaded path goes
//      into the draft, as a dropped file's does.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, createEvent, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  }),
  daemonCliPost: vi.fn(),
  executeRemoteDrop: vi.fn(),
  executeBrowserFileDrop: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  getLocalDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  invalidateDaemonWs: () => {},
  daemonWsBase: () => 'ws://127.0.0.1:1',
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))
vi.mock('@/lib/pick-compose-files', () => ({
  composeAttachPlan: () => ({ kind: 'native' }),
  pickLocalComposeFiles: vi.fn(async () => null),
  pickRemoteComposeFile: vi.fn(async () => null),
}))
vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: h.executeRemoteDrop,
  executeBrowserFileDrop: h.executeBrowserFileDrop,
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))

import { TerminalComposeBar } from './TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const WS = '/Users/me/ai/sales'
const DOWNLOADS = `${WS}/.k2/downloads`
const ON = ['thread-attachments-v1']

function scopeOf(features: string[]): ServerScope {
  return { ...fakeScope('local', { remote: false }), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

const chrome: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function renderBar(opts: { features?: string[]; dest?: 'thread' | 'pty' } = {}): HTMLTextAreaElement {
  const scope = scopeOf(opts.features ?? ON)
  const room = testRoom({
    tabs: {},
    scope,
    isPrimary: true,
    projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <TerminalComposeBar sessionId="sess-1" workspacePath={WS} sendDestination={opts.dest ?? 'thread'} />
    </SessionViewChromeContext.Provider>,
  )
  return screen.getByRole('textbox') as HTMLTextAreaElement
}

/** Browser files land in the workspace's downloads, under their names. */
function uploadsBrowserFiles(_s: ServerScope, files: File[], _t: unknown, _c: unknown, build?: (r: string[]) => string | null) {
  const remote = files.map((f) => `${DOWNLOADS}/${f.name}`)
  return Promise.resolve(build ? build(remote) : null)
}

/** Paste with this clipboard; true when the field's own paste still runs. */
function paste(box: HTMLTextAreaElement, clip: { files?: File[]; items?: File[]; text?: string }): boolean {
  const items = (clip.items ?? []).map((f) => ({ kind: 'file', type: f.type, getAsFile: () => f }))
  const ev = createEvent.paste(box, {
    clipboardData: {
      files: clip.files ?? [],
      items,
      types: [...(clip.text ? ['text/plain'] : []), ...((clip.files?.length ?? 0) + items.length > 0 ? ['Files'] : [])],
      getData: (t: string) => (t === 'text/plain' ? clip.text ?? '' : ''),
    },
  })
  let ran = true
  act(() => {
    ran = fireEvent(box, ev)
  })
  return ran
}

function chipNames(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
}

function threadPosts(): Array<Record<string, unknown>> {
  return h.daemonCliPost.mock.calls.filter((c) => c[1] === 'thread/post').map((c) => c[2] as Record<string, unknown>)
}

function uploadedNames(): string[] {
  return h.executeBrowserFileDrop.mock.calls.flatMap((c) => (c[1] as File[]).map((f) => f.name))
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockClear()
  h.executeRemoteDrop.mockReset()
  h.executeBrowserFileDrop.mockReset()
  h.executeBrowserFileDrop.mockImplementation(uploadsBrowserFiles)
})

afterEach(() => {
  cleanup()
})

describe('pasting into the agent Thread box', () => {
  it('an image item (no name) uploads as a browser file, becomes a chip, and rides the send as an attachment', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9 })
    const box = renderBar()
    const shot = new File(['png-bytes'], '', { type: 'image/png' })
    expect(paste(box, { items: [shot] }), 'the field drops nothing of its own').toBe(false)
    await waitFor(() => expect(chipNames()).toHaveLength(1))
    const name = chipNames()[0]
    expect(name).toMatch(/^pasted-image-\d{8}-\d{6}\.png$/)
    expect(uploadedNames()).toEqual([name])
    // The Thread's attach path: a browser file upload into downloads,
    // attached as a file (any name), never typed.
    expect(h.executeBrowserFileDrop).toHaveBeenCalledTimes(1)
    expect(h.executeBrowserFileDrop.mock.calls[0][3]).toEqual({ workspacePath: WS, attachesFiles: true })
    expect(box.value).toBe('')
    // The picture's chip draws it.
    expect(document.querySelector('[data-compose-chip-thumb]')).not.toBeNull()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    })
    expect(threadPosts()).toHaveLength(1)
    expect(threadPosts()[0].attachments).toEqual([{ path: `${DOWNLOADS}/${name}` }])
    expect(threadPosts()[0].text).toBe('')
  })

  it('a file item (WKWebView files, named) is a chip by its own name', async () => {
    const box = renderBar()
    const pdf = new File(['%PDF'], 'brief.pdf', { type: 'application/pdf' })
    expect(paste(box, { files: [pdf], text: 'brief.pdf' }), 'a Finder copy: the name text does not paste').toBe(false)
    await waitFor(() => expect(chipNames()).toEqual(['brief.pdf']))
    expect(uploadedNames()).toEqual(['brief.pdf'])
    expect(box.value).toBe('')
  })

  it('two screenshots pasted one after the other upload under different names', async () => {
    const box = renderBar()
    paste(box, { files: [new File(['a'], 'image.png', { type: 'image/png' })] })
    await waitFor(() => expect(chipNames()).toHaveLength(1))
    // A second later (each paste names its picture by the time).
    vi.useFakeTimers({ toFake: ['Date'] })
    try {
      vi.setSystemTime(Date.now() + 1500)
      paste(box, { files: [new File(['b'], 'image.png', { type: 'image/png' })] })
    } finally {
      vi.useRealTimers()
    }
    await waitFor(() => expect(chipNames()).toHaveLength(2))
    const names = uploadedNames()
    expect(names).toHaveLength(2)
    expect(names[0]).not.toBe(names[1])
  })

  it('text only: the field pastes it, nothing uploads', () => {
    const box = renderBar()
    expect(paste(box, { text: 'just words' }), 'default paste runs').toBe(true)
    expect(h.executeBrowserFileDrop).not.toHaveBeenCalled()
    expect(h.executeRemoteDrop).not.toHaveBeenCalled()
    expect(chipNames()).toEqual([])
  })

  it('an image plus real text: the chip, and the text still pastes', async () => {
    const box = renderBar()
    const shot = new File(['png'], 'image.png', { type: 'image/png' })
    expect(paste(box, { files: [shot], text: 'see the chart' }), 'the text pastes as text').toBe(true)
    await waitFor(() => expect(chipNames()).toHaveLength(1))
    expect(chipNames()[0]).toMatch(/^pasted-image-.*\.png$/)
  })
})

describe('pasting into a box without Thread files', () => {
  it('the PTY bar: the image uploads and its path goes into the draft, as a drop does', async () => {
    const box = renderBar({ dest: 'pty' })
    expect(paste(box, { files: [new File(['png'], 'image.png', { type: 'image/png' })] })).toBe(false)
    await waitFor(() => expect(box.value).toMatch(new RegExp(`^${DOWNLOADS}/pasted-image-\\d{8}-\\d{6}\\.png $`)))
    expect(chipNames()).toEqual([])
  })
})
