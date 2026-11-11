// @vitest-environment jsdom
// thread-send-resilience (2026-11-11): a message typed on a remote Home
// room, sent right after the laptop woke, was lost: the box cleared on
// Enter, the room remounted under its failure gate while the send was out,
// and the words had nowhere to come back to. A send is now in the Thread
// outbox (on disk) before the saved draft goes; a refusal brings the words
// back to the box even after a remount; without a disk the saved draft
// stays until the server has the message.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, waitFor } from '@testing-library/react'
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
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { primaryRoom } from '@/stores/room'
import { scopedKey } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { configureOutboxForTests, outboxEntriesFor, outboxKey, resetOutboxForTests } from '@/components/SessionView/threadOutbox'
import { clearPendingSendsForTests, readPendingSends, resetThreadViewCacheForTests } from '@/components/SessionView/threadViewCache'

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

function Chrome({ children }: { children: ReactNode }) {
  return <SessionViewChromeContext.Provider value={CHROME}>{children}</SessionViewChromeContext.Provider>
}

function bar() {
  return <TerminalComposeBar sessionId="sess-1" draftId="proj-1" workspacePath="/ws/sales" sendDestination="thread" />
}

function draftKey(): string {
  return scopedKey(primaryRoom().scope, 'k2:composer:draft:agent:proj-1:thread')
}

function box(): HTMLTextAreaElement {
  const ta = document.querySelector('[data-compose-bar] textarea')
  if (!ta) throw new Error('no message box')
  return ta as HTMLTextAreaElement
}

function threadPosts(): Array<Record<string, unknown>> {
  return (h.daemonCliPost.mock.calls as unknown[][]).filter((c) => c[0] === 'thread/post').map((c) => c[1] as Record<string, unknown>)
}

/** The next thread/post waits for the test. */
function gatePost(): { resolve: (v: unknown) => void; reject: (e: unknown) => void } {
  const g = {} as { resolve: (v: unknown) => void; reject: (e: unknown) => void }
  h.daemonCliPost.mockImplementationOnce(
    () =>
      new Promise((res, rej) => {
        g.resolve = res
        g.reject = rej
      }),
  )
  return g
}

function send(words: string): void {
  fireEvent.change(box(), { target: { value: words } })
  fireEvent.keyDown(box(), { key: 'Enter' })
}

describe('the Thread box keeps a message until it is safe', () => {
  beforeEach(async () => {
    cleanup()
    localStorage.clear()
    ;(globalThis as { indexedDB?: IDBFactory }).indexedDB = new IDBFactory()
    await resetThreadViewCacheForTests({ keepDisk: true })
    configureOutboxForTests({ autoTimers: false })
    resetOutboxForTests()
    h.daemonCliPost.mockReset()
    h.daemonCliPost.mockImplementation(async () => ({ ok: true }))
  })
  afterEach(async () => {
    cleanup()
    resetOutboxForTests()
    await clearPendingSendsForTests()
  })

  it('a refusal that lands after the box was remounted (a room failure gate) brings the words back, saved', async () => {
    const r = renderInPrimaryRoom(<Chrome>{bar()}</Chrome>)
    const gate = gatePost()
    send('typed before the lid closed')
    await waitFor(() => expect(threadPosts()).toHaveLength(1))
    expect(box().value).toBe('')
    // The room remounts while the send is out.
    r.unmount()
    renderInPrimaryRoom(<Chrome>{bar()}</Chrome>)
    await act(async () => {
      gate.reject(new DaemonHttpError('You can’t post here any more.', 403, 'forbidden'))
    })
    await waitFor(() => expect(box().value).toBe('typed before the lid closed'))
    expect(localStorage.getItem(draftKey())).toBe('typed before the lid closed')
    // Back in the box, not also waiting in the Thread.
    expect(outboxEntriesFor(outboxKey(primaryRoom().scope, 'sales'))).toEqual([])
  })

  it('with nowhere on disk for the outbox, the saved draft stays until the server has the message', async () => {
    ;(globalThis as { indexedDB?: IDBFactory }).indexedDB = undefined
    await resetThreadViewCacheForTests({ keepDisk: true })
    renderInPrimaryRoom(<Chrome>{bar()}</Chrome>)
    const gate = gatePost()
    send('keep me until it lands')
    await waitFor(() => expect(threadPosts()).toHaveLength(1))
    expect(box().value).toBe('')
    expect(localStorage.getItem(draftKey())).toBe('keep me until it lands')
    const cid = threadPosts()[0].clientMsgId as string
    await act(async () => {
      gate.resolve({ ok: true, id: 'own-1', seq: 4, body: 'keep me until it lands', via: 'compose', clientMsgId: cid })
    })
    await waitFor(() => expect(localStorage.getItem(draftKey())).toBeNull())
    expect(box().value).toBe('')
  })

  it('with a disk, the saved draft goes once the message is in the outbox on disk (before any answer)', async () => {
    renderInPrimaryRoom(<Chrome>{bar()}</Chrome>)
    gatePost() // never answered
    send('on disk first')
    await waitFor(() => expect(threadPosts()).toHaveLength(1))
    await waitFor(async () => expect((await readPendingSends()).map((r) => r.text)).toEqual(['on disk first']))
    await waitFor(() => expect(localStorage.getItem(draftKey())).toBeNull())
  })

  it('a send that fails on the way: the box stays empty and the message waits in the Thread, on disk', async () => {
    renderInPrimaryRoom(<Chrome>{bar()}</Chrome>)
    h.daemonCliPost.mockImplementationOnce(async () => {
      throw new TypeError('Load failed')
    })
    send('waits for the server')
    await waitFor(() => expect(outboxEntriesFor(outboxKey(primaryRoom().scope, 'sales')).map((e) => e.state)).toEqual(['waiting']))
    expect(box().value).toBe('')
    await waitFor(async () => expect((await readPendingSends()).map((r) => [r.text, r.state])).toEqual([['waits for the server', 'waiting']]))
  })
})
