// @vitest-environment jsdom
// The chat's model in the composer, and who else is typing under it.
//
//   1. The model is quiet text left of the stash button, server default
//      included; the old chip (only off the default) is gone.
//   2. Like the stash button it shows only while the box is empty: text or
//      files hide it, an empty box brings it back.
//   3. A click, or `/model` typed (shown or hidden), opens the picker.
//   4. Under the box, the typing sliver: always there at one height (the
//      box never moves), its words for one, two and several people.
//   5. Terminal view's square bar has both; a bare terminal has no sliver.
//   6. Typing in the box says so on the Thread's socket: only while the box
//      has text, never on blur.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  options: { sessionId: 'sess-1', harness: 'claude', switchable: true, current: null as string | null, serverDefault: 'opus' as string | null, workspaceDefault: null },
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(async () => ({ ok: true })),
  picked: [] as string[][],
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
  pickLocalComposeFiles: vi.fn(async () => h.picked.shift() ?? null),
  pickRemoteComposeFile: vi.fn(async () => null),
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))

import { TerminalComposeBar } from './TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { THREAD_TYPING_SLIVER_PX } from '@/components/Thread/ThreadTypingSliver'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useContextMenuStore } from '@/stores/context-menu'
import { noteThreadTyping, parseThreadTypingFrame, registerThreadTypingSocket, resetThreadTypingForTests } from '@/lib/thread-typing'

const WS = '/Users/me/ai/sales'
const FEATURES = ['thread-attachments-v1', 'thread-typing-v1']

function scopeOf(features = FEATURES): ServerScope {
  return { ...fakeScope('box', { remote: false }), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

const chrome = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function renderBar(opts: { dest?: 'thread' | 'pty'; chrome?: boolean; scope?: ServerScope } = {}): void {
  const room = testRoom({
    tabs: {},
    scope: opts.scope ?? scopeOf(),
    isPrimary: true,
    projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
  const bar = <TerminalComposeBar sessionId="sess-1" workspacePath={WS} sendDestination={opts.dest ?? 'thread'} />
  renderInRoom(room, opts.chrome === false ? bar : <SessionViewChromeContext.Provider value={chrome}>{bar}</SessionViewChromeContext.Provider>)
}

const box = (): HTMLTextAreaElement => screen.getByRole('textbox') as HTMLTextAreaElement
const model = (): HTMLElement | null => screen.queryByTestId('compose-model')
const stash = (): HTMLElement | null => screen.queryByTestId('stash-button')

function typing(...names: string[]): void {
  act(() => {
    names.forEach((name, i) => {
      noteThreadTyping('box', parseThreadTypingFrame({ collection: 'typing', seq: 0, id: 'sales', member: `m${i}`, name, kind: 'person', at: 0 })!)
    })
  })
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  resetThreadTypingForTests()
  h.options.current = null
  h.options.serverDefault = 'opus'
  h.options.harness = 'claude'
  h.picked.length = 0
  h.daemonCliPost.mockClear()
  h.daemonCliGet.mockReset()
  h.daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'sessions/model/options') return { ...h.options }
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  })
})

afterEach(() => {
  cleanup()
  resetThreadTypingForTests()
})

describe('the model in the Thread box', () => {
  it('names the server default, left of the stash button, with no chip', async () => {
    renderBar()
    await waitFor(() => expect(model()?.textContent).toBe('Opus 5.5'))
    expect(model()!.getAttribute('aria-label')).toBe('Model for this chat: Opus 5.5. Change')
    // Left of the stash button, which is left of Send.
    const row = [...model()!.parentElement!.querySelectorAll('[data-testid="compose-model"], [data-testid="stash-button"], [data-zen-send]')]
    expect(row.map((el) => el.getAttribute('data-testid') ?? 'send')).toEqual(['compose-model', 'stash-button', 'send'])
    expect(screen.queryByTestId('compose-model-chip')).toBeNull()
  })

  it('names a switched model, and still no chip', async () => {
    h.options.current = 'haiku'
    renderBar()
    await waitFor(() => expect(model()?.textContent).toBe('Haiku 5.5'))
    expect(screen.queryByTestId('compose-model-chip')).toBeNull()
  })

  it('names no model in a shell', async () => {
    h.options.harness = 'zsh'
    renderBar()
    await waitFor(() => expect(h.daemonCliGet.mock.calls.some((c) => c[1] === 'sessions/model/options')).toBe(true))
    await act(async () => {})
    expect(model()).toBeNull()
    expect(stash()).not.toBeNull()
  })

  it('hides with text, as the stash button does, and comes back on an empty box', async () => {
    renderBar()
    await waitFor(() => expect(model()).not.toBeNull())
    fireEvent.change(box(), { target: { value: 'Can you add the kickoff call?' } })
    expect(model()).toBeNull()
    expect(stash()).toBeNull()
    fireEvent.change(box(), { target: { value: '' } })
    expect(model()).not.toBeNull()
    expect(stash()).not.toBeNull()
  })

  it('hides with files in the box', async () => {
    renderBar()
    await waitFor(() => expect(model()).not.toBeNull())
    h.picked.push([`${WS}/docs/plan.md`])
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Attach or save' }))
    })
    await act(async () => {
      useContextMenuStore.getState().selectItem('attach')
    })
    await waitFor(() => expect(document.querySelector('[data-zen-attachment]')).not.toBeNull())
    expect(box().value).toBe('')
    expect(model()).toBeNull()
    expect(stash()).toBeNull()
  })

  it('opens the picker on a click', async () => {
    renderBar()
    await waitFor(() => expect(model()).not.toBeNull())
    fireEvent.click(model()!)
    expect(await screen.findByTestId('compose-model-picker')).toBeTruthy()
  })

  it('/model opens the picker while the model is hidden', async () => {
    renderBar()
    await waitFor(() => expect(model()).not.toBeNull())
    fireEvent.change(box(), { target: { value: 'half a thought' } })
    expect(model()).toBeNull()
    fireEvent.change(box(), { target: { value: '/model ' } })
    expect(await screen.findByTestId('compose-model-picker')).toBeTruthy()
  })
})

describe('the typing sliver under the Thread box', () => {
  const sliver = (): HTMLElement => screen.getByTestId('thread-typing-sliver')
  const words = (): string => sliver().querySelector('[data-typing-words]')!.textContent ?? ''
  const spoken = (): string => sliver().querySelector('[aria-live="polite"]')!.textContent ?? ''

  it('is there, empty, at its one height while nobody types', () => {
    renderBar()
    expect(sliver().style.height).toBe(`${THREAD_TYPING_SLIVER_PX}px`)
    expect(words()).toBe('')
    expect(spoken()).toBe('')
  })

  it('fills in place: the same row at the same height, under the same box', () => {
    renderBar()
    const row = sliver()
    const boxEl = document.querySelector('[data-zen-compose-box]')
    expect(boxEl?.nextElementSibling).toBe(row)
    typing('Mira')
    expect(sliver()).toBe(row)
    expect(row.style.height).toBe(`${THREAD_TYPING_SLIVER_PX}px`)
    expect(document.querySelector('[data-zen-compose-box]')).toBe(boxEl)
    expect(row.querySelector('[data-typing-words]')!.textContent).toBe('Mira is typing…')
  })

  it('says one, two, and several people', () => {
    renderBar()
    typing('Mira')
    expect(words()).toBe('Mira is typing…')
    expect(spoken()).toBe('Mira is typing…')
    act(() => resetThreadTypingForTests())
    typing('Mira', 'Ben')
    expect(words()).toBe('Mira and Ben are typing…')
    act(() => resetThreadTypingForTests())
    typing('Mira', 'Ben', 'Ana')
    expect(words()).toBe('Several people are typing…')
  })

  it("says this person's typing on the Thread's socket", async () => {
    const sent: string[] = []
    const release = registerThreadTypingSocket('box', () => 'sales', (f) => {
      sent.push(f)
      return true
    })
    renderBar()
    fireEvent.change(box(), { target: { value: 'H' } })
    fireEvent.change(box(), { target: { value: 'Hi' } })
    expect(sent).toEqual([JSON.stringify({ t: 'typing', thread: 'sales' })])
    release()
  })

  it('says nothing for an emptied box, and nothing on blur', () => {
    const sent: string[] = []
    const release = registerThreadTypingSocket('box', () => 'sales', (f) => {
      sent.push(f)
      return true
    })
    renderBar()
    fireEvent.focus(box())
    fireEvent.change(box(), { target: { value: '' } })
    fireEvent.blur(box())
    expect(sent).toEqual([])
    release()
  })

  it('says nothing to a server without thread-typing-v1', () => {
    const sent: string[] = []
    const release = registerThreadTypingSocket('box', () => 'sales', (f) => {
      sent.push(f)
      return true
    })
    renderBar({ scope: scopeOf(['thread-attachments-v1']) })
    fireEvent.change(box(), { target: { value: 'Hi' } })
    expect(sent).toEqual([])
    release()
  })
})

describe("Terminal view's square bar", () => {
  it('names the model left of the stash button, hidden with text', async () => {
    renderBar({ dest: 'pty' })
    await waitFor(() => expect(model()?.getAttribute('data-compose-model')).toBe('square'))
    expect(model()!.nextElementSibling).toBe(stash())
    fireEvent.change(box(), { target: { value: 'ls' } })
    expect(model()).toBeNull()
    fireEvent.change(box(), { target: { value: '' } })
    expect(model()).not.toBeNull()
  })

  it('has the typing sliver in its own styling', () => {
    renderBar({ dest: 'pty' })
    const row = screen.getByTestId('thread-typing-sliver')
    expect(row.getAttribute('data-typing-sliver')).toBe('square')
    typing('Mira')
    expect(row.querySelector('[data-typing-words]')!.textContent).toBe('Mira is typing…')
  })

  it('a bare terminal (no agent chat) has no sliver', () => {
    renderBar({ dest: 'pty', chrome: false })
    expect(screen.queryByTestId('thread-typing-sliver')).toBeNull()
  })
})
