// @vitest-environment jsdom
// The Thread's message box before the session is ready (Rosson 2026-11-10:
// "Even if the terminal hasn't loaded yet … The user should be able to type
// there"): editable at once, the text, caret and focus kept when the
// session comes up (in place, or across the remount from the pinned Chat's
// starting body to its terminal pane), sent through the Thread route
// without a session, and kept when a send or the start fails.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'

const h = vi.hoisted(() => {
  const daemonCliGet = vi.fn(async (route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  })
  const daemonCliPost = vi.fn(async (_route: string, _body?: unknown): Promise<unknown> => ({ ok: true }))
  return { daemonCliGet, daemonCliPost }
})

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(h.daemonCliGet),
    daemonCliPost: primaryOnly(h.daemonCliPost),
  }
})

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

import { TerminalComposeBar } from './TerminalComposeBar'
import { composeWaitingLabel } from './terminalCompose'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { primaryRoom } from '@/stores/room'
import { scopedKey } from '@/kessel/server-scope'

const CHROME: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'proj-1',
  displayName: 'sales',
  projectPath: '/ws/sales',
}

function Chrome({ children, addr = 'sales' }: { children: ReactNode; addr?: string }) {
  return <SessionViewChromeContext.Provider value={{ ...CHROME, overlayAddr: addr }}>{children}</SessionViewChromeContext.Provider>
}

/** The box as the pinned Chat draws it: the early box and the terminal
 *  pane's both pass the pane's v2 agent name (the workspace id) as draftId. */
function bar(sessionId: string, draftId: string | null = 'proj-1') {
  return <TerminalComposeBar sessionId={sessionId} draftId={draftId ?? undefined} workspacePath="/ws/sales" sendDestination="thread" />
}

function draftKey(): string {
  return scopedKey(primaryRoom().scope, 'k2:composer:draft:agent:proj-1:thread')
}

function box(): HTMLTextAreaElement {
  const ta = document.querySelector('[data-compose-bar] textarea')
  if (!ta) throw new Error('no message box')
  return ta as HTMLTextAreaElement
}

function threadPosts(): unknown[][] {
  return (h.daemonCliPost.mock.calls as unknown[][]).filter((c) => c[0] === 'thread/post')
}

describe('composeWaitingLabel', () => {
  it('the waiting Send names the agent', () => {
    expect(composeWaitingLabel('sales')).toBe('Sends when sales is ready')
    expect(composeWaitingLabel('  ')).toBe('Sends when the agent is ready')
  })
})

describe('the Thread box before the session is ready', () => {
  beforeEach(() => {
    cleanup()
    localStorage.clear()
    h.daemonCliPost.mockReset()
    h.daemonCliPost.mockImplementation(async () => ({ ok: true }))
  })
  afterEach(() => cleanup())

  it('a bar with no owner yet keeps what was typed when it gets one', () => {
    const r = renderInPrimaryRoom(<Chrome>{bar('', null)}</Chrome>)
    fireEvent.change(box(), { target: { value: 'typed before any key' } })
    r.rerender(<Chrome>{bar('sess-9', null)}</Chrome>)
    expect(box().value).toBe('typed before any key')
    expect(localStorage.getItem(scopedKey(primaryRoom().scope, 'k2:composer:draft:sess-9:thread'))).toBe('typed before any key')
  })

  it('is editable with no session, and saves under the pane’s draft', () => {
    renderInPrimaryRoom(<Chrome>{bar('')}</Chrome>)
    const ta = box()
    expect(ta.disabled).toBe(false)
    expect(ta.readOnly).toBe(false)
    ta.focus()
    fireEvent.change(ta, { target: { value: 'Can you check the build' } })
    expect(ta.value).toBe('Can you check the build')
    expect(localStorage.getItem(draftKey())).toBe('Can you check the build')
    expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false)
  })

  it('keeps the text, caret and focus when the session comes up in place', () => {
    const r = renderInPrimaryRoom(<Chrome>{bar('')}</Chrome>)
    const ta = box()
    ta.focus()
    fireEvent.change(ta, { target: { value: 'half a thought' } })
    ta.setSelectionRange(4, 4)
    r.rerender(<Chrome>{bar('sess-1')}</Chrome>)
    expect(box()).toBe(ta)
    expect(ta.value).toBe('half a thought')
    expect(document.activeElement).toBe(ta)
    expect(ta.selectionStart).toBe(4)
    expect(localStorage.getItem(draftKey())).toBe('half a thought')
  })

  it('keeps the text, caret and focus across the remount to the terminal pane’s box', () => {
    // The pinned Chat: its starting body draws one box, the terminal pane
    // (another tree) draws the next, with a session.
    const r = renderInPrimaryRoom(
      <Chrome>
        <div data-testid="starting">{bar('')}</div>
      </Chrome>,
    )
    const first = box()
    first.focus()
    fireEvent.change(first, { target: { value: 'typed while it started' } })
    first.setSelectionRange(5, 9)
    r.rerender(
      <Chrome>
        <section data-testid="ready">{bar('')}</section>
      </Chrome>,
    )
    const next = box()
    expect(next).not.toBe(first)
    expect(screen.getByTestId('ready').contains(next)).toBe(true)
    expect(next.value).toBe('typed while it started')
    expect(document.activeElement).toBe(next)
    expect([next.selectionStart, next.selectionEnd]).toEqual([5, 9])
    // …and the session id arriving on the new box changes nothing.
    r.rerender(
      <Chrome>
        <section data-testid="ready">{bar('sess-1')}</section>
      </Chrome>,
    )
    expect(box()).toBe(next)
    expect(next.value).toBe('typed while it started')
    expect(document.activeElement).toBe(next)
  })

  it('a box that did not have the caret does not take it on the remount', () => {
    const r = renderInPrimaryRoom(
      <Chrome>
        <button type="button" data-testid="elsewhere">elsewhere</button>
        <div>{bar('')}</div>
      </Chrome>,
    )
    fireEvent.change(box(), { target: { value: 'saved words' } })
    const other = screen.getByTestId('elsewhere')
    other.focus()
    r.rerender(
      <Chrome>
        <button type="button" data-testid="elsewhere">elsewhere</button>
        <section>{bar('sess-1')}</section>
      </Chrome>,
    )
    expect(box().value).toBe('saved words')
    expect(document.activeElement).toBe(other)
  })

  it('a box unmounted with the caret hands nothing to a box mounted later', async () => {
    const first = renderInPrimaryRoom(<Chrome>{bar('')}</Chrome>)
    box().focus()
    fireEvent.change(box(), { target: { value: 'gone with its tab' } })
    first.unmount()
    // The draft went to storage; someone else (a send elsewhere, another
    // window) cleared it before this pane opens again.
    localStorage.clear()
    await Promise.resolve()
    renderInPrimaryRoom(
      <Chrome>
        <button type="button" data-testid="elsewhere">elsewhere</button>
        {bar('')}
      </Chrome>,
    )
    expect(box().value).toBe('')
    expect(document.activeElement).not.toBe(box())
  })

  it('sends through the Thread route with no session (it needs the address only)', async () => {
    renderInPrimaryRoom(<Chrome>{bar('')}</Chrome>)
    fireEvent.change(box(), { target: { value: 'ship it' } })
    fireEvent.keyDown(box(), { key: 'Enter' })
    await waitFor(() =>
      expect(threadPosts()).toEqual([
        ['thread/post', expect.objectContaining({ addr: 'sales', text: 'ship it', via: 'compose' })],
      ]),
    )
    await waitFor(() => expect(box().value).toBe(''))
  })

  it('no address yet: Send waits with the draft and goes by itself once the address comes', async () => {
    const r = renderInPrimaryRoom(<Chrome addr="">{bar('')}</Chrome>)
    fireEvent.change(box(), { target: { value: 'review the diff' } })
    fireEvent.keyDown(box(), { key: 'Enter' })
    const waiting = screen.getByRole('button', { name: 'Sends when sales is ready' })
    expect(waiting.hasAttribute('disabled')).toBe(true)
    expect(screen.getByTestId('compose-thread-waiting').textContent).toBe('Sends when sales is ready.')
    expect(box().value).toBe('review the diff')
    expect(threadPosts()).toEqual([])
    // Keep typing while it waits: the newest words go.
    fireEvent.change(box(), { target: { value: 'review the diff, then merge' } })
    r.rerender(<Chrome addr="sales/reviewer">{bar('sess-2')}</Chrome>)
    await waitFor(() =>
      expect(threadPosts()).toEqual([
        ['thread/post', expect.objectContaining({ addr: 'sales/reviewer', text: 'review the diff, then merge' })],
      ]),
    )
    await waitFor(() => expect(box().value).toBe(''))
    expect(screen.queryByTestId('compose-thread-waiting')).toBeNull()
  })

  it('a refused send keeps the draft and says why', async () => {
    h.daemonCliPost.mockImplementation(async () => ({ ok: false, error: 'The agent is not taking messages.' }))
    renderInPrimaryRoom(<Chrome>{bar('')}</Chrome>)
    fireEvent.change(box(), { target: { value: 'are you there' } })
    fireEvent.keyDown(box(), { key: 'Enter' })
    await waitFor(() => expect(screen.getByTestId('compose-thread-addr-error').textContent).toBe('The agent is not taking messages.'))
    expect(box().value).toBe('are you there')
    expect(localStorage.getItem(draftKey())).toBe('are you there')
  })

  it('a start that fails keeps the draft: the box comes back with it', () => {
    const r = renderInPrimaryRoom(<Chrome>{bar('')}</Chrome>)
    fireEvent.change(box(), { target: { value: 'still here after the error' } })
    // The pane shows its error in place of the Thread column…
    r.rerender(<Chrome><div>Chat session failed to start</div></Chrome>)
    expect(document.querySelector('[data-compose-bar] textarea')).toBeNull()
    // …and Retry brings the box back.
    r.rerender(<Chrome>{bar('sess-3')}</Chrome>)
    expect(box().value).toBe('still here after the error')
  })

  it.each([
    ['its PTY session', 'k2:composer:draft:sess-1:thread'],
    ['a preview build’s pane key', 'k2:composer:draft:thread:/ws/sales|proj-1'],
  ])('a Thread draft saved under %s comes over once, with its files', (_what, oldKey) => {
    const legacy = scopedKey(primaryRoom().scope, oldKey)
    localStorage.setItem(legacy, 'words from before')
    localStorage.setItem(`${legacy}:files`, JSON.stringify([{ path: '/ws/sales/docs/plan.md' }]))
    renderInPrimaryRoom(<Chrome>{bar('sess-1')}</Chrome>)
    expect(box().value).toBe('words from before')
    expect(localStorage.getItem(legacy)).toBeNull()
    expect(localStorage.getItem(`${legacy}:files`)).toBeNull()
    expect(localStorage.getItem(draftKey())).toBe('words from before')
    expect(JSON.parse(localStorage.getItem(`${draftKey()}:files`) ?? '[]')).toEqual([{ path: '/ws/sales/docs/plan.md' }])
  })

  it('an old key never overwrites words already in the box', () => {
    localStorage.setItem(draftKey(), 'newer words')
    localStorage.setItem(scopedKey(primaryRoom().scope, 'k2:composer:draft:sess-1:thread'), 'older words')
    renderInPrimaryRoom(<Chrome>{bar('sess-1')}</Chrome>)
    expect(box().value).toBe('newer words')
  })
})
