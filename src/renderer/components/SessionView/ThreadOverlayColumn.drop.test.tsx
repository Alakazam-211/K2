// @vitest-environment jsdom
// A file dropped anywhere on the Thread pane attaches to its message box
// (Rosson 2026-11-16), through the box's own drop handler: the same chips,
// image previews, limits and refusals as a drop on the box. Fail loud.
//
//   1. A drop on the message list (or the empty pane) attaches; the drop
//      is taken (no navigation) and the pane shows one drop state while
//      the file is over it.
//   2. A drag with no files (text, a link, an in-window drag) does nothing.
//   3. A pane whose box takes no drop (view-only room, no box) offers none.
//   4. Crossing child elements never flickers the drop state off.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  }),
  daemonCliPost: vi.fn(),
  executeRemoteDrop: vi.fn(),
  executeBrowserFileDrop: vi.fn(),
  threadHook: {
    items: [] as unknown[],
    error: null as string | null,
    posting: false,
    post: async () => {},
    answer: async () => {},
    voidCard: async () => {},
    hasMore: false,
    loadingOlder: false,
    loaded: true,
    outbox: [] as unknown[],
    loadOlder: async () => {},
  },
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
vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: h.executeRemoteDrop,
  executeBrowserFileDrop: h.executeBrowserFileDrop,
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))
vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => h.threadHook,
}))

import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { fakeScope } from '@/test-utils/fake-scope'
import { inRoom, renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { SessionViewChromeContext, type SessionViewChromeValue } from './sessionViewChrome'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'

const WS = '/Users/me/ai/sales'
const ON = ['thread-attachments-v1']

function scopeOf(features: string[]): ServerScope {
  return { ...fakeScope('local', { remote: false }), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

const chrome = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function roomOf(readOnly: boolean) {
  return testRoom({
    tabs: {},
    scope: scopeOf(ON),
    readOnly,
    projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
}

function column(withBar = true) {
  return (
    <SessionViewChromeContext.Provider value={chrome}>
      <ThreadOverlayColumn
        addr="sales"
        conversationId={null}
        active
        composeBar={withBar ? <TerminalComposeBar sessionId="sess-1" workspacePath={WS} sendDestination="thread" /> : null}
      />
    </SessionViewChromeContext.Provider>
  )
}

/** A file from the OS, with its path (the desktop shell's `File.path`). */
function osFile(path: string, type = ''): File {
  const name = path.split('/').pop() ?? path
  return Object.assign(new File(['x'], name, { type }), { path })
}

function filesDrag(files: File[]) {
  return {
    dataTransfer: {
      types: ['Files'],
      files: Object.assign([...files], { item: (i: number) => files[i] ?? null }),
      dropEffect: 'none',
      getData: () => '',
    },
  }
}

function textDrag() {
  return {
    dataTransfer: {
      types: ['text/plain', 'text/uri-list'],
      files: Object.assign([], { item: () => null }),
      dropEffect: 'none',
      getData: (t: string) => (t === 'text/plain' ? 'https://example.test/page' : ''),
    },
  }
}

const pane = () => screen.getByTestId('agent-session-thread')
const list = () => screen.getByTestId('agent-session-thread-list-slot')
const overlay = () => screen.queryByTestId('thread-drop-overlay')
const chipNames = (): string[] =>
  Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
const box = () => screen.getByRole('textbox') as HTMLTextAreaElement

/** A real element deep in the message list (a child the pointer crosses). */
function listChild(): HTMLElement {
  const all = list().querySelectorAll<HTMLElement>('*')
  expect(all.length).toBeGreaterThan(1)
  return all[all.length - 1]
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockClear()
  h.executeRemoteDrop.mockReset()
  h.executeBrowserFileDrop.mockReset()
})

afterEach(() => {
  cleanup()
})

describe('a file dropped anywhere on the Thread pane attaches', () => {
  it('a drop on the message list attaches the files, with the image preview, and is taken', async () => {
    h.executeRemoteDrop.mockImplementation(
      async (_s: ServerScope, local: string[], _t: unknown, _c: unknown, build?: (r: string[]) => string | null) => {
        const remote = local.map((p) => `${WS}/.k2/downloads/${p.split('/').pop()}`)
        return build ? build(remote) : null
      },
    )
    renderInRoom(roomOf(false), column())
    const files = [osFile(`${WS}/out/hero.png`, 'image/png'), osFile('/Users/me/Desktop/notes.pdf', 'application/pdf')]

    fireEvent.dragEnter(list(), filesDrag(files))
    expect(overlay(), 'the pane shows its drop state').not.toBeNull()
    expect(overlay()!.textContent).toBe('Drop to attach')
    expect(pane().hasAttribute('data-thread-drop-over')).toBe(true)
    // dragover is taken, so the browser fires drop (and never opens the file).
    expect(fireEvent.dragOver(list(), filesDrag(files))).toBe(false)

    let taken = true
    await act(async () => {
      taken = !fireEvent.drop(list(), filesDrag(files))
    })
    expect(taken, 'the drop is taken: no navigation, no other pane').toBe(true)
    expect(overlay()).toBeNull()
    await waitFor(() => expect(chipNames()).toEqual(['hero.png', 'notes.pdf']))
    // The image chip has its picture (the box's own preview); no path text.
    expect(document.querySelector('[data-compose-chip-thumb]')).not.toBeNull()
    expect(box().value).toBe('')
    // Same path as a drop on the box: only the outside file was uploaded.
    expect(h.executeRemoteDrop).toHaveBeenCalledTimes(1)
    expect(h.executeRemoteDrop.mock.calls[0][1]).toEqual(['/Users/me/Desktop/notes.pdf'])
  })

  it('a drop on the pane itself (the empty Thread, under the header) attaches too', async () => {
    renderInRoom(roomOf(false), column())
    const files = [osFile(`${WS}/docs/plan.md`, 'text/markdown')]
    fireEvent.dragEnter(pane(), filesDrag(files))
    await act(async () => {
      fireEvent.drop(pane(), filesDrag(files))
    })
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
  })

  it('a drag with no files (text, a link) does nothing', async () => {
    renderInRoom(roomOf(false), column())
    expect(fireEvent.dragEnter(list(), textDrag()), 'not taken').toBe(true)
    expect(overlay()).toBeNull()
    expect(fireEvent.dragOver(list(), textDrag()), 'dragover not taken').toBe(true)
    let notTaken = false
    await act(async () => {
      notTaken = fireEvent.drop(list(), textDrag())
    })
    expect(notTaken, 'the drop is not taken').toBe(true)
    expect(overlay()).toBeNull()
    expect(chipNames()).toEqual([])
    expect(box().value).toBe('')
  })

  it('a pane whose box takes no drop (view-only room, or no box) offers none', async () => {
    const files = [osFile(`${WS}/out/hero.png`, 'image/png')]
    const { rerender } = renderInRoom(roomOf(false), column())
    // Takes drops while the box does …
    fireEvent.dragEnter(list(), filesDrag(files))
    expect(overlay()).not.toBeNull()
    fireEvent.dragLeave(list(), { ...filesDrag(files), relatedTarget: null })
    expect(overlay()).toBeNull()

    // … a view-only room renders no box: the pane refuses.
    rerender(inRoom(roomOf(true), column()))
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(fireEvent.dragEnter(list(), filesDrag(files)), 'dragenter not taken').toBe(true)
    expect(fireEvent.dragOver(list(), filesDrag(files)), 'dragover not taken').toBe(true)
    expect(overlay()).toBeNull()
    expect(pane().hasAttribute('data-thread-drop-over')).toBe(false)
    let notTaken = false
    await act(async () => {
      notTaken = fireEvent.drop(list(), filesDrag(files))
    })
    expect(notTaken).toBe(true)

    // No box at all (an imported Thread): none either.
    rerender(inRoom(roomOf(false), column(false)))
    expect(fireEvent.dragOver(list(), filesDrag(files))).toBe(true)
    fireEvent.dragEnter(list(), filesDrag(files))
    expect(overlay()).toBeNull()
  })

  it('crossing child elements keeps one steady drop state (no flicker)', () => {
    renderInRoom(roomOf(false), column())
    const files = [osFile(`${WS}/out/hero.png`, 'image/png')]
    const child = listChild()
    const shown: boolean[] = []
    const look = () => shown.push(overlay() !== null)

    // Into the pane, then onto a child: enter (child) fires before leave
    // (list), as browsers do.
    fireEvent.dragEnter(list(), filesDrag(files))
    look()
    fireEvent.dragEnter(child, filesDrag(files))
    look()
    fireEvent.dragLeave(list(), { ...filesDrag(files), relatedTarget: child })
    look()
    // Back off the child onto the list.
    fireEvent.dragEnter(list(), filesDrag(files))
    look()
    fireEvent.dragLeave(child, { ...filesDrag(files), relatedTarget: list() })
    look()
    // A browser that reports no relatedTarget: the depth count holds it.
    fireEvent.dragEnter(child, filesDrag(files))
    look()
    fireEvent.dragLeave(list(), { ...filesDrag(files), relatedTarget: null })
    look()
    expect(shown).toEqual([true, true, true, true, true, true, true])
    // Out of the pane: off.
    fireEvent.dragLeave(child, { ...filesDrag(files), relatedTarget: document.body })
    expect(overlay()).toBeNull()
    expect(screen.queryAllByTestId('thread-drop-overlay')).toHaveLength(0)
  })
})
