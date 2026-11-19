// @vitest-environment jsdom
// A file attached to a Thread's message box: the box is ready to type in
// (caret at the end of any words), and Return sends the files even with
// no text. Fail loud.
//
//   1. A pick, and a drop the window routed: the box takes the caret, at
//      the end of the words already there.
//   2. A dialog that opened while the file was on its way keeps the caret
//      off the box.
//   3. Return with a file and no words sends the file.
//   4. Return with no file and no words sends nothing, as before.
//   5. Shift+Return is a new line (no send, the key is left to the box).
//   6. Return while an IME is composing sends nothing.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  }),
  daemonCliPost: vi.fn(),
  pick: vi.fn<() => Promise<string[] | null>>(),
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
  pickLocalComposeFiles: h.pick,
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
import { useContextMenuStore } from '@/stores/context-menu'

const WS = '/Users/me/ai/sales'
const ON = ['thread-attachments-v1']

function scope(): ServerScope {
  return { ...fakeScope('local', { remote: false }), serverSupports: (f: string) => ON.includes(f) } as ServerScope
}

const chrome: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function renderBar(): HTMLTextAreaElement {
  const room = testRoom({
    tabs: {},
    scope: scope(),
    isPrimary: true,
    projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <TerminalComposeBar sessionId="sess-1" workspacePath={WS} sendDestination="thread" />
    </SessionViewChromeContext.Provider>,
  )
  return screen.getByRole('textbox') as HTMLTextAreaElement
}

async function openPicker(): Promise<void> {
  // "+" opens its menu (attach, then Save to stash); pick attach.
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Attach or save' }))
  })
  await act(async () => {
    useContextMenuStore.getState().selectItem('attach')
  })
}

async function pick(paths: string[]): Promise<void> {
  h.pick.mockImplementationOnce(async () => paths)
  await openPicker()
}

function nextFrame(): Promise<void> {
  return act(async () => {
    await new Promise<void>((r) => requestAnimationFrame(() => r()))
  })
}

function chipNames(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
}

function threadPosts(): unknown[] {
  return h.daemonCliPost.mock.calls.filter((c) => c[1] === 'thread/post').map((c) => c[2])
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  document.querySelectorAll('[data-test-dialog]').forEach((el) => el.remove())
  h.daemonCliPost.mockReset()
  h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9 })
  h.pick.mockReset()
  h.executeRemoteDrop.mockReset()
  h.executeBrowserFileDrop.mockReset()
})

afterEach(() => {
  cleanup()
  document.querySelectorAll('[data-test-dialog]').forEach((el) => el.remove())
})

describe('after a file attaches, the box is ready to type in', () => {
  it('a pick: the box takes the caret, at the end of the words already there', async () => {
    const box = renderBar()
    fireEvent.change(box, { target: { value: 'Have a look' } })
    // The person left the caret at the start, then clicked away (the box
    // keeps that caret for its next focus).
    act(() => {
      box.focus()
      box.setSelectionRange(0, 0)
      box.blur()
    })
    expect(document.activeElement).not.toBe(box)
    await pick([`${WS}/docs/plan.md`])
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
    await waitFor(() => expect(document.activeElement).toBe(box))
    expect([box.selectionStart, box.selectionEnd]).toEqual([11, 11])
    expect(box.value).toBe('Have a look')
  })

  it('a drop the window routed: the box takes the caret too', async () => {
    const box = renderBar()
    expect(document.activeElement).not.toBe(box)
    const bar = document.querySelector('[data-compose-bar]') as HTMLElement
    await act(async () => {
      bar.dispatchEvent(
        new CustomEvent('k2:compose-insert', {
          detail: { data: `'${WS}/a.png' `, paths: [`${WS}/a.png`], uploaded: true },
        }),
      )
    })
    await waitFor(() => expect(chipNames()).toEqual(['a.png']))
    await waitFor(() => expect(document.activeElement).toBe(box))
    expect([box.selectionStart, box.selectionEnd]).toEqual([0, 0])
  })

  it('a dialog that opened while the file was on its way: the caret stays off the box', async () => {
    const box = renderBar()
    let release: (paths: string[]) => void = () => {
      throw new Error('the picker never opened')
    }
    h.pick.mockImplementationOnce(() => new Promise<string[]>((r) => (release = r)))
    await openPicker()
    expect(h.pick).toHaveBeenCalledTimes(1)
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'dialog')
    dialog.setAttribute('data-test-dialog', '')
    document.body.appendChild(dialog)
    await act(async () => {
      release([`${WS}/docs/plan.md`])
    })
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
    await nextFrame()
    await nextFrame()
    expect(document.activeElement).not.toBe(box)
  })
})

describe('Return with files', () => {
  it('Return with a file and no words sends the file', async () => {
    const box = renderBar()
    await pick([`${WS}/docs/plan.md`])
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
    expect(box.value).toBe('')
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(threadPosts()).toEqual([
      {
        addr: 'sales',
        text: '',
        via: 'compose',
        attachments: [{ path: `${WS}/docs/plan.md` }],
      },
    ])
    await waitFor(() => expect(chipNames()).toEqual([]))
  })

  it('Return with no file and no words sends nothing', async () => {
    const box = renderBar()
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(threadPosts()).toEqual([])
  })

  it('Shift+Return is a new line: no send, the key is left to the box', async () => {
    const box = renderBar()
    await pick([`${WS}/docs/plan.md`])
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
    let notPrevented = false
    await act(async () => {
      notPrevented = fireEvent.keyDown(box, { key: 'Enter', shiftKey: true })
    })
    expect(notPrevented).toBe(true)
    expect(threadPosts()).toEqual([])
    expect(chipNames()).toEqual(['plan.md'])
  })

  it('Return while an IME is composing sends nothing', async () => {
    const box = renderBar()
    await pick([`${WS}/docs/plan.md`])
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter', isComposing: true })
    })
    expect(threadPosts()).toEqual([])
    expect(chipNames()).toEqual(['plan.md'])
  })
})
