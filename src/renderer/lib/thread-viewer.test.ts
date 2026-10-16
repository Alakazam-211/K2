import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'
import { cardExpiryLabel, threadItemsFromSnapshot, overlayItemFromThreadPost } from '@/components/SessionView/overlayThread'

const calls = vi.hoisted(() => ({
  whoami: [] as unknown[],
  prefs: [] as unknown[],
  log: [] as string[],
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async (_scope: unknown, route: string) => {
    calls.log.push(route)
    if (route !== 'auth/whoami') throw new Error(`unexpected route ${route}`)
    const next = calls.whoami.shift()
    if (next instanceof Error) throw next
    if (next === undefined) throw new Error('test gave no whoami answer')
    return next
  },
  daemonCliPost: async () => {
    throw new Error('no posts in this test')
  },
}))

vi.mock('@/components/Settings/sections/notify-api', () => ({
  fetchNotifyPrefs: async () => {
    calls.log.push('notify/prefs')
    const next = calls.prefs.shift()
    if (next instanceof Error) throw next
    if (next === undefined) throw new Error('test gave no prefs answer')
    return next
  },
}))

vi.mock('@/stores/session-events', () => ({
  onNotifyPrefsChanged: () => () => {},
}))

import {
  cardAnswererLabel,
  guessThreadViewer,
  isOwnThreadPost,
  loadThreadViewer,
  parseThreadAuthor,
  resetThreadViewersForTests,
  threadViewerNow,
} from './thread-viewer'

function fakeScope(opts: { key: string; remote: boolean; username?: string; push?: boolean }): ServerScope {
  return {
    id: opts.key,
    isPrimary: false,
    hostKey: opts.key,
    connectionKey: opts.key,
    isRemote: opts.remote,
    label: opts.key,
    connectHost: () => (opts.username === undefined ? null : ({ username: opts.username } as never)),
    isWindowHost: () => false,
    creds: async () => {
      throw new Error('no creds in this test')
    },
    httpBase: async () => 'http://test.invalid',
    wsBase: async () => 'ws://test.invalid',
    serverSupports: (f: string) => f === 'thread-push-v1' && opts.push === true,
  } as unknown as ServerScope
}

afterEach(() => {
  resetThreadViewersForTests()
  calls.whoami = []
  calls.prefs = []
  calls.log = []
  vi.restoreAllMocks()
})

describe('parseThreadAuthor', () => {
  it('takes {kind, login} and refuses anything else', () => {
    expect(parseThreadAuthor({ kind: 'user', login: ' bob ' })).toEqual({ kind: 'user', login: 'bob' })
    expect(parseThreadAuthor({ kind: 'owner', login: 'owner', extra: 1 })).toEqual({ kind: 'owner', login: 'owner' })
    for (const bad of [null, undefined, 'bob', [], {}, { kind: 'user' }, { login: 'bob' }, { kind: '', login: 'b' }, { kind: 'user', login: 3 }]) {
      expect(parseThreadAuthor(bad)).toBeNull()
    }
  })
})

describe('isOwnThreadPost', () => {
  const owner = { principal: 'owner', aliases: ['rosson'] }
  const alice = { principal: 'alice', aliases: [] }
  const doc = (author?: unknown, via = 'compose') => ({ via, author })

  it('owner-token posts are the owner\'s only', () => {
    expect(isOwnThreadPost(doc({ kind: 'owner', login: 'owner' }), owner)).toBe(true)
    expect(isOwnThreadPost(doc({ kind: 'owner', login: 'owner' }), alice)).toBe(false)
  })

  it('a Connect login is its own person (case-insensitive), or the owner when linked', () => {
    expect(isOwnThreadPost(doc({ kind: 'user', login: 'ALICE' }), alice)).toBe(true)
    expect(isOwnThreadPost(doc({ kind: 'user', login: 'bob' }), alice)).toBe(false)
    expect(isOwnThreadPost(doc({ kind: 'user', login: 'Rosson' }), owner)).toBe(true)
    expect(isOwnThreadPost(doc({ kind: 'user', login: 'alice' }), owner)).toBe(false)
  })

  it('guests and unknown kinds are never the viewer; nobody owns posts for an unknown viewer', () => {
    expect(isOwnThreadPost(doc({ kind: 'guest', login: 'alice' }, 'thread'), alice)).toBe(false)
    expect(isOwnThreadPost(doc({ kind: 'robot', login: 'alice' }), alice)).toBe(false)
    expect(isOwnThreadPost(doc({ kind: 'owner', login: 'owner' }), null)).toBe(false)
  })

  it('a post that came with a clone is never "You", even for the owner on the source login', () => {
    const imported = (login: string) => ({ via: 'import', author: { kind: 'imported', login, name: `${login} (Connect, src.k2.dev)` } })
    expect(isOwnThreadPost(imported('owner'), owner)).toBe(false)
    expect(isOwnThreadPost(imported('alice'), alice)).toBe(false)
    expect(isOwnThreadPost(imported('rosson'), owner)).toBe(false)
    expect(cardAnswererLabel({ kind: 'imported', login: 'owner', name: 'Rosson' }, owner)).toBe('Rosson')
    expect(cardAnswererLabel({ kind: 'imported', login: 'alice' }, alice)).toBe('alice')
  })

  it('a doc without an author keeps the old via=compose rule', () => {
    expect(isOwnThreadPost(doc(undefined), alice)).toBe(true)
    expect(isOwnThreadPost(doc(undefined), null)).toBe(true)
    expect(isOwnThreadPost(doc(undefined, 'thread'), owner)).toBe(false)
  })
})

describe('the viewer', () => {
  it('guesses the owner on this computer, the saved login on a remote, nobody on a bare web host', () => {
    expect(guessThreadViewer(fakeScope({ key: 'local', remote: false }))).toEqual({ principal: 'owner', aliases: [] })
    expect(guessThreadViewer(fakeScope({ key: 'r1', remote: true, username: ' alice ' }))).toEqual({ principal: 'alice', aliases: [] })
    expect(guessThreadViewer(fakeScope({ key: 'r2', remote: true }))).toBeNull()
  })

  it('asks notify/prefs on a thread-push server (principal + owner aliases), once per connection', async () => {
    const scope = fakeScope({ key: 'p1', remote: true, username: 'rosson', push: true })
    calls.prefs.push({ principal: 'owner', aliases: ['rosson'] })
    const first = loadThreadViewer(scope)
    const second = loadThreadViewer(scope)
    expect(await first).toEqual({ principal: 'owner', aliases: ['rosson'] })
    expect(await second).toEqual({ principal: 'owner', aliases: ['rosson'] })
    expect(await loadThreadViewer(scope)).toEqual({ principal: 'owner', aliases: ['rosson'] })
    expect(threadViewerNow(scope)).toEqual({ principal: 'owner', aliases: ['rosson'] })
    expect(calls.log).toEqual(['notify/prefs'])
  })

  it('asks whoami on an older server (owner token, or a Connect username)', async () => {
    calls.whoami.push({ owner: true, username: null })
    expect(await loadThreadViewer(fakeScope({ key: 'w1', remote: true }))).toEqual({ principal: 'owner', aliases: [] })
    calls.whoami.push({ owner: false, username: 'bob' })
    expect(await loadThreadViewer(fakeScope({ key: 'w2', remote: true }))).toEqual({ principal: 'bob', aliases: [] })
    expect(calls.log).toEqual(['auth/whoami', 'auth/whoami'])
  })

  it('falls back to whoami when notify/prefs fails, and to the guess when both fail', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    calls.prefs.push(new Error('prefs down'))
    calls.whoami.push({ owner: false, username: 'carol' })
    expect(await loadThreadViewer(fakeScope({ key: 'f1', remote: true, push: true }))).toEqual({
      principal: 'carol',
      aliases: [],
    })
    calls.whoami.push(new Error('whoami down'))
    const scope = fakeScope({ key: 'f2', remote: true, username: 'dave' })
    expect(await loadThreadViewer(scope)).toEqual({ principal: 'dave', aliases: [] })
    // A failure is not retried at once (no request storm).
    expect(await loadThreadViewer(scope)).toEqual({ principal: 'dave', aliases: [] })
    expect(calls.log).toEqual(['notify/prefs', 'auth/whoami', 'auth/whoami'])
    expect(warn).toHaveBeenCalledTimes(2)
  })
})

describe('Thread docs keep author and widget', () => {
  it('from a snapshot and from a post answer', () => {
    const snap = threadItemsFromSnapshot({
      conversation_id: 'c',
      items: [
        {
          seq: 1,
          id: 'a',
          collection: 'thread',
          doc: {
            id: 'a',
            kind: 'text',
            from: 'bob',
            via: 'compose',
            body: 'hi',
            author: { kind: 'user', login: 'bob' },
            widget: { widget: 'Agent Arcade', garden: 'g-test0001' },
          },
        },
        { seq: 2, id: 'b', collection: 'thread', doc: { id: 'b', kind: 'text', from: 'sales', via: 'thread', body: 'yo' } },
      ],
    })
    expect(snap.items[0].doc.author).toEqual({ kind: 'user', login: 'bob' })
    expect(snap.items[0].doc.widget).toEqual({ widget: 'Agent Arcade', garden: 'g-test0001' })
    expect('author' in snap.items[1].doc).toBe(false)
    expect('widget' in snap.items[1].doc).toBe(false)

    const posted = overlayItemFromThreadPost(
      { ok: true, id: 'p', seq: 3, from: 'Rosson', via: 'compose', body: 'x', author: { kind: 'owner', login: 'owner' }, widget: null },
      'x',
    )
    if (!posted) throw new Error('post answer did not parse')
    expect(posted.doc.author).toEqual({ kind: 'owner', login: 'owner' })
    expect('widget' in posted.doc).toBe(false)
  })
})

describe('cardAnswererLabel', () => {
  const owner = { principal: 'owner', aliases: ['rosson'] }
  const alice = { principal: 'alice', aliases: [] }
  it('says You for the viewer, the owner for the owner token, else the login', () => {
    expect(cardAnswererLabel({ kind: 'owner', login: 'owner' }, owner)).toBe('You')
    expect(cardAnswererLabel({ kind: 'user', login: 'rosson' }, owner)).toBe('You')
    expect(cardAnswererLabel({ kind: 'user', login: 'alice' }, alice)).toBe('You')
    expect(cardAnswererLabel({ kind: 'owner', login: 'owner' }, alice)).toBe('the owner')
    expect(cardAnswererLabel({ kind: 'guest', login: 'patty' }, owner)).toBe('patty')
    expect(cardAnswererLabel({ kind: 'guest', login: 'alice' }, alice)).toBe('alice')
  })
  it("shows the owner's display name, never a bare owner", () => {
    const rosson = { kind: 'owner', login: 'owner', name: 'Rosson Long' }
    expect(cardAnswererLabel(rosson, alice)).toBe('Rosson Long')
    expect(cardAnswererLabel(rosson, null)).toBe('Rosson Long')
    expect(cardAnswererLabel(rosson, owner)).toBe('You')
    expect(cardAnswererLabel({ kind: 'owner', login: 'owner', name: ' ' }, alice)).toBe('the owner')
    expect(cardAnswererLabel({ kind: 'owner', login: 'owner', name: 'owner' }, alice)).toBe('the owner')
    // A name on any other kind is ignored: the login is who it was.
    expect(cardAnswererLabel({ kind: 'guest', login: 'patty', name: 'Rosson' }, owner)).toBe('patty')
    expect(parseThreadAuthor({ kind: 'owner', login: 'owner', name: ' Rosson ' })).toEqual({ kind: 'owner', login: 'owner', name: 'Rosson' })
  })
  it('names nobody for a card without answeredBy (older cards render as before)', () => {
    expect(cardAnswererLabel(undefined, owner)).toBeNull()
    expect(cardAnswererLabel(null, owner)).toBeNull()
    expect(cardAnswererLabel({ kind: '', login: 'x' }, owner)).toBeNull()
  })
})

describe('card answeredBy coercion', () => {
  it('keeps answeredBy/answeredAt from the daemon and leaves older cards alone', () => {
    const { items } = threadItemsFromSnapshot({
      items: [
        {
          collection: 'thread',
          seq: 1,
          id: 'c1',
          doc: {
            id: 'c1',
            kind: 'choice',
            from: 'k2',
            choice: { prompt: '?', options: [{ label: 'Go' }], allow_custom: false, status: 'answered', answer: 'Go', answeredBy: { kind: 'guest', login: 'g1' }, answeredAt: 42 },
          },
        },
        {
          collection: 'thread',
          seq: 2,
          id: 'c2',
          doc: { id: 'c2', kind: 'choice', from: 'k2', choice: { prompt: '?', options: [{ label: 'Go' }], allow_custom: false, status: 'answered', answer: 'Go' } },
        },
      ],
    })
    expect(items[0].doc.choice?.answeredBy).toEqual({ kind: 'guest', login: 'g1' })
    expect(items[0].doc.choice?.answeredAt).toBe(42)
    expect(items[1].doc.choice && 'answeredBy' in items[1].doc.choice).toBe(false)
  })
})

describe('cardExpiryLabel', () => {
  it('says how long a card has, expired once its time passed, nothing without a time frame', () => {
    const now = 1_000_000
    expect(cardExpiryLabel(undefined, now)).toBeNull()
    expect(cardExpiryLabel(null, now)).toBeNull()
    expect(cardExpiryLabel(now, now)).toBe('expired')
    expect(cardExpiryLabel(now + 30, now)).toBe('expires in 1 minute')
    expect(cardExpiryLabel(now + 90 * 60, now)).toBe('expires in 2 hours')
    expect(cardExpiryLabel(now + 3600, now)).toBe('expires in 1 hour')
    expect(cardExpiryLabel(now + 7 * 86400, now)).toBe('expires in 7 days')
  })
  it('keeps expiresAt through coercion', () => {
    const { items } = threadItemsFromSnapshot({
      items: [{ collection: 'thread', seq: 1, id: 'c', doc: { id: 'c', kind: 'choice', from: 'k2', choice: { prompt: '?', options: ['Go'], allow_custom: false, status: 'pending', expiresAt: 99 } } }],
    })
    expect(items[0].doc.choice?.expiresAt).toBe(99)
  })
})
