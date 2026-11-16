// @vitest-environment jsdom
// An OS file drop on the desktop Thread's message box, end to end through
// the window's drop router (routeExternalDrop: the macOS path, where Tauri
// hands the drop to the page as tauri://drag-drop + a point, never as an
// HTML5 drop) into the real TerminalComposeBar, for a Thread on another
// server (Rosson 2026-11-15: "I keep drag-and-dropping images into the
// Thread … but the images don't append to my message"; the files had
// reached the other server's .k2/downloads, and nothing said why).
//
//   1. The point lands on any part of the box, the reply bar (its words and
//      its ×) or the Jump to bottom pill over the messages: the image is a
//      chip.
//   2. The box is redrawn while the upload runs (a new root element for the
//      same session): the chip lands in the new box.
//   3. The box is gone when the upload answers: a toast says the file
//      reached the server but no box took it. Never a silent drop.
// Fail loud.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  }),
  daemonCliPost: vi.fn(),
  executeRemoteDrop: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.daemonCliGet, daemonCliPost: h.daemonCliPost }))
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
  executeBrowserFileDrop: vi.fn(),
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))

import type { ReactElement } from 'react'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { ThreadJumpPill } from '@/components/Thread/ThreadJumpPill'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import { resetThreadReplyForTests, threadReplyKey, useThreadReplyStore } from '@/lib/thread-reply'
import { composeDropNotTakenMessage, routeExternalDrop } from './external-drop-router'

const WS = '/Users/me/ai/sales'
const SHOT = '/Users/r/Desktop/Screenshot 2026-11-15 at 9.15.08 PM.png'
const UPLOADED = `${WS}/.k2/downloads/Screenshot 2026-11-15 at 9.15.08 PM.png`
const CHIP = 'Screenshot 2026-11-15 at 9.15.08 PM.png'

const scope = {
  ...fakeScope('remote:rosson', { remote: true }),
  serverSupports: (f: string) => f === 'thread-attachments-v1',
} as ServerScope

const chrome = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

const room = testRoom({
  tabs: {},
  scope,
  isPrimary: false,
  projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
  activeProjectId: 'p',
})

/** The Thread column as ThreadOverlayColumn draws it: the drop surface,
 *  the messages with the Jump to bottom pill, and the box. */
function column(opts: { box?: boolean; boxKey?: string } = {}): ReactElement {
  return (
    <SessionViewChromeContext.Provider value={chrome}>
      <div data-compose-drop-surface="thread">
        <div data-testid="messages">
          <ThreadJumpPill shown unseen={2} sent={false} reducedMotion onJump={() => {}} bottom={0} />
        </div>
        {opts.box === false ? null : (
          <TerminalComposeBar key={opts.boxKey ?? 'a'} sessionId="sess-1" workspacePath={WS} sendDestination="thread" />
        )}
      </div>
    </SessionViewChromeContext.Provider>
  )
}

/** The page's element at the drop point (jsdom has no layout). */
function pointAt(el: Element): void {
  ;(document as unknown as { elementFromPoint: (x: number, y: number) => Element | null }).elementFromPoint = () => el
}

/** The upload to the room's server answers right away (the upload itself
 *  is handle-remote-drop's; this is what it hands back). */
function uploadsNow(): void {
  h.executeRemoteDrop.mockImplementation(
    async (_s: ServerScope, local: string[], _t: unknown, _c: unknown, build?: (r: string[]) => string | null) =>
      build ? build(local.map((p) => `${WS}/.k2/downloads/${p.split('/').pop()}`)) : null,
  )
}

/** The upload answers when the test says so. */
function uploadsLater(): () => void {
  let release: () => void = () => {
    throw new Error('the upload never started')
  }
  h.executeRemoteDrop.mockImplementation(
    (_s: ServerScope, local: string[], _t: unknown, _c: unknown, build?: (r: string[]) => string | null) =>
      new Promise<string | null>((resolve) => {
        release = () => resolve(build ? build(local.map((p) => `${WS}/.k2/downloads/${p.split('/').pop()}`)) : null)
      }),
  )
  return () => release()
}

function chips(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
}

function toasts(): string[] {
  return useToastStore.getState().toasts.map((t) => t.message)
}

async function drop(): Promise<void> {
  await act(async () => {
    await routeExternalDrop(scope, [SHOT], { x: 400, y: 800 })
  })
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  resetThreadReplyForTests()
  useToastStore.setState({ toasts: [] })
  h.executeRemoteDrop.mockReset()
  h.daemonCliPost.mockReset()
})

afterEach(() => {
  cleanup()
})

describe('an image dropped on the Thread of an agent on another server', () => {
  it.each([
    ['the message field', () => document.querySelector('textarea')!],
    ['the box itself', () => document.querySelector('[data-zen-compose-box]')!],
    ['the Jump to bottom pill', () => document.querySelector('[data-testid="messages"] button')!],
    ['the messages', () => document.querySelector('[data-testid="messages"]')!],
  ])('lands on %s: uploaded to that server and a chip in the box', async (_where, at) => {
    renderInRoom(room, column())
    uploadsNow()
    pointAt(at())
    await drop()
    expect(h.executeRemoteDrop).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(chips()).toEqual([CHIP]))
    expect(toasts().filter((m) => m.includes("didn't take"))).toEqual([])
  })

  it.each([
    ['its words', '[data-reply-chip-text]'],
    ['its ×', '[data-reply-cancel]'],
  ])('lands on the reply bar (%s): a chip, and the reply stays', async (_where, sel) => {
    renderInRoom(room, column())
    act(() => {
      useThreadReplyStore.getState().set(threadReplyKey(scope, 'sales'), {
        id: 'm42',
        seq: 42,
        from: 'sales',
        addr: 'sales',
        text: 'Ship it?',
        part: false,
      })
    })
    const bar = document.querySelector('[data-thread-reply-bar]')
    expect(bar, 'the reply bar shows').not.toBeNull()
    uploadsNow()
    pointAt(document.querySelector(sel)!)
    await drop()
    await waitFor(() => expect(chips()).toEqual([CHIP]))
    expect(document.querySelector('[data-thread-reply-bar]')).not.toBeNull()
  })

  it('the box is redrawn while the upload runs: the chip lands in the new box', async () => {
    const view = renderInRoom(room, column({ boxKey: 'a' }))
    const release = uploadsLater()
    const before = document.querySelector('[data-compose-bar]')
    pointAt(document.querySelector('textarea')!)
    let routed!: Promise<void>
    act(() => {
      routed = routeExternalDrop(scope, [SHOT], { x: 400, y: 800 })
    })
    view.rerender(column({ boxKey: 'b' }))
    const after = document.querySelector('[data-compose-bar]')
    expect(after).not.toBeNull()
    expect(after, 'a new root element for the same box').not.toBe(before)
    await act(async () => {
      release()
      await routed
    })
    await waitFor(() => expect(chips()).toEqual([CHIP]))
    expect(toasts().filter((m) => m.includes("didn't take"))).toEqual([])
  })

  it('the box is gone when the upload answers: a toast says so', async () => {
    const view = renderInRoom(room, column())
    const release = uploadsLater()
    pointAt(document.querySelector('textarea')!)
    let routed!: Promise<void>
    act(() => {
      routed = routeExternalDrop(scope, [SHOT], { x: 400, y: 800 })
    })
    view.rerender(column({ box: false }))
    await act(async () => {
      release()
      await routed
    })
    expect(chips()).toEqual([])
    expect(toasts()).toContain(composeDropNotTakenMessage([UPLOADED], true))
    expect(composeDropNotTakenMessage([UPLOADED], true)).toBe(
      "Screenshot 2026-11-15 at 9.15.08 PM.png reached the agent's server, but the message box didn't take it. Drop it again.",
    )
  })
})
