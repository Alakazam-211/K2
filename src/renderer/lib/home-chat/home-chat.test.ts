// People and channels on Home: the pure rules (prd-home-people-channels-v1
// §6, §9, §10, §11, §13).

import { describe, expect, it } from 'vitest'
import { addressFitsKind, channelAddress, homeChatTarget, isPersonExt, personAddress } from './address'
import { compareSewIds, parseHomeChatMessage, parseHomeChatPage, parseHomeChatPeople, parseHomeChatSummary } from './api'
import {
  QUICK_REACTIONS,
  applyReactionFrame,
  authorName,
  cachedHomeChat,
  markDeleted,
  memberNames,
  mergeHomeChatMessages,
  rememberHomeChat,
  replyPreview,
  resetHomeChatViewCacheForTests,
  searchEmoji,
  toggleReaction,
  typingLine,
  VIEW_CACHE_CHATS,
} from './chat'
import { parseHomeChatFrame } from './events-socket'
import { matchesElsewhere, mightBeSamePerson, pickablePerson } from './people'
import { callerIsAdmin, channelPerms } from './permissions'
import type { HomeChatMessage, HomeChatPerson } from './api'

function msg(id: string, extra: Partial<HomeChatMessage> = {}): HomeChatMessage {
  return { id, conv: '9', authorId: '100', body: `m${id}`, createdAt: 1, seq: 1, reactions: [], replyTo: null, deleted: false, nonce: null, ...extra }
}

function person(ext: string, extra: Partial<HomeChatPerson> = {}): HomeChatPerson {
  return { ext, username: ext.split(':').pop() ?? '', displayName: '', picture: null, role: null, ready: true, memberId: null, ...extra }
}

describe('addresses', () => {
  it('a person is a box person, a channel a Sew id', () => {
    expect(personAddress('K2:Connect:3F2A', 'Box.example.test')).toBe('k2:connect:3f2a::box.example.test')
    expect(personAddress('k2:workspace:5d1c', 'box')).toBeNull()
    expect(personAddress('k2:principal:a', 'box')).toBeNull()
    expect(isPersonExt('k2:owner')).toBe(true)
    expect(channelAddress('184467', 'box')).toBe('184467::box')
    expect(channelAddress('general', 'box')).toBeNull()
    expect(addressFitsKind('channel', 'atlas::box')).toBe(false)
    expect(addressFitsKind('agent', 'atlas::box')).toBe(true)
    expect(homeChatTarget({ kind: 'person', address: 'k2:owner::local' })).toEqual({ kind: 'person', hostKey: 'local', ext: 'k2:owner', address: 'k2:owner::local' })
    expect(homeChatTarget({ address: 'atlas::local' })).toBeNull()
  })
})

describe('Sew answers', () => {
  it('reads a message: reactions (Unicode only), the reply reference, a tombstone', () => {
    const m = parseHomeChatMessage({
      id: '12',
      channel_id: '9',
      author_id: '100',
      body: 'hi @alex',
      seq: 4,
      created_at: '2026-11-14T10:00:00Z',
      reactions: [
        { emoji: { kind: 'unicode', value: '👍' }, count: 2, me: true },
        { emoji: { kind: 'custom', id: '5', name: 'party' }, count: 1, me: false },
      ],
      reference: { kind: 'reply', message_id: '10', channel_id: '9' },
    })
    expect(m).toMatchObject({ id: '12', body: 'hi @alex', replyTo: '10', reactions: [{ emoji: '👍', count: 2, me: true }], deleted: false })
    expect(m?.createdAt).toBe(Date.parse('2026-11-14T10:00:00Z') / 1000)
    expect(parseHomeChatMessage({ id: '1', channel_id: '9', reference: { kind: 'quote', message_id: '3' } })?.replyTo).toBeNull()
    expect(parseHomeChatMessage({ id: '1', channel_id: '9', deleted_at: '2026-11-14T10:00:00Z' })?.deleted).toBe(true)
  })

  it('a page is oldest first by id, numerically', () => {
    const p = parseHomeChatPage({ page: { items: [{ id: '100', channel_id: '9' }, { id: '99', channel_id: '9' }, { id: '1000', channel_id: '9' }], has_more: true } })
    expect(p.messages.map((m) => m.id)).toEqual(['99', '100', '1000'])
    expect(p.hasMore).toBe(true)
    expect(compareSewIds('9', '10')).toBeLessThan(0)
  })

  it('summary badges and the DM person; people with member ids', () => {
    const s = parseHomeChatSummary({
      chat: 'live',
      convs: [
        { conv: '5', kind: 'dm', unread: true, badge: { kind: 'count', n: 3 }, with: 'k2:connect:a' },
        { conv: '6', kind: 'channel', unread: true, badge: { kind: 'dot' }, inviteOnly: true },
        { conv: '7', kind: 'channel', unread: false, badge: { kind: 'none' }, inviteOnly: false },
      ],
    })
    expect(s.convs.map((c) => c.inviteOnly)).toEqual([null, true, false])
    expect(s.convs.map((c) => [c.conv, c.badge, c.with])).toEqual([
      ['5', { kind: 'count', n: 3 }, 'k2:connect:a'],
      ['6', { kind: 'dot' }, null],
      ['7', { kind: 'none' }, null],
    ])
    const p = parseHomeChatPeople({ me: { ext: 'k2:connect:me', memberId: '1' }, people: [{ ext: 'k2:owner', username: 'owner', displayName: 'Rae', ready: true, memberId: '2', role: 'owner' }] })
    expect(p.me).toEqual({ ext: 'k2:connect:me', memberId: '1' })
    expect(p.people[0]).toMatchObject({ ext: 'k2:owner', displayName: 'Rae', memberId: '2', role: 'owner' })
  })

  it('events frames', () => {
    expect(parseHomeChatFrame(JSON.stringify({ type: 'typing', conv: '9', member: '100' }))).toEqual({ type: 'typing', conv: '9', member: '100' })
    expect(parseHomeChatFrame(JSON.stringify({ type: 'reaction', on: true, conv: '9', msg: '12', emoji: { kind: 'unicode', value: '🎉' }, count: 1, member: '100', mine: false }))).toMatchObject({ type: 'reaction', emoji: '🎉', count: 1 })
    expect(parseHomeChatFrame('nope')).toBeNull()
    expect(parseHomeChatFrame(JSON.stringify({ type: 'call_ring' }))).toBeNull()
  })
})

describe('reactions (§10)', () => {
  it('a tap toggles the viewer’s own: add, then remove; the chip goes at zero', () => {
    let list = [msg('1')]
    const on = toggleReaction(list, '1', '👍')
    expect(on.on).toBe(true)
    list = on.messages
    expect(list[0].reactions).toEqual([{ emoji: '👍', count: 1, me: true }])
    const off = toggleReaction(list, '1', '👍')
    expect(off.on).toBe(false)
    expect(off.messages[0].reactions).toEqual([])
  })

  it('a live frame sets the count; `me` moves only for the viewer’s own', () => {
    let list = [msg('1', { reactions: [{ emoji: '👍', count: 1, me: true }] })]
    list = applyReactionFrame(list, { msg: '1', emoji: '👍', on: true, count: 2, mine: false })
    expect(list[0].reactions).toEqual([{ emoji: '👍', count: 2, me: true }])
    list = applyReactionFrame(list, { msg: '1', emoji: '❤️', on: true, count: 1, mine: false })
    expect(list[0].reactions[1]).toEqual({ emoji: '❤️', count: 1, me: false })
    list = applyReactionFrame(list, { msg: '1', emoji: '👍', on: false, count: 1, mine: true })
    expect(list[0].reactions[0]).toEqual({ emoji: '👍', count: 1, me: false })
  })

  it('the picker has six quick emoji and searches the Unicode set', () => {
    expect(QUICK_REACTIONS).toEqual(['👍', '❤️', '😂', '🎉', '😮', '🙏'])
    expect(searchEmoji('')).toEqual([...QUICK_REACTIONS])
    expect(searchEmoji('rock')).toContain('🚀')
    expect(searchEmoji('thumbs up')).toContain('👍')
    expect(searchEmoji('zzzz-nothing')).toEqual([])
  })
})

describe('replies, names and typing', () => {
  it('a reply’s preview: loaded, deleted, or not loaded', () => {
    const names = memberNames(parseHomeChatPeople({ me: { ext: 'k2:owner', memberId: '1' }, people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: '100' }] }))
    expect(replyPreview(msg('1', { body: '\nfirst line\nsecond' }), '1', names)).toEqual({ state: 'loaded', author: 'Alex', line: 'first line' })
    expect(replyPreview(msg('1', { deleted: true }), '1', names)).toEqual({ state: 'deleted' })
    expect(replyPreview(undefined, '1', names)).toEqual({ state: 'missing' })
    expect(authorName('1', '1', names)).toBe('You')
    expect(authorName('999', '1', names)).toBe('Someone')
  })

  it('who is typing reads as a sentence', () => {
    expect(typingLine([])).toBe('')
    expect(typingLine(['Alex'])).toBe('Alex is typing…')
    expect(typingLine(['Alex', 'Sam'])).toBe('Alex and Sam are typing…')
    expect(typingLine(['Alex', 'Sam', 'Kim'])).toBe('Alex and 2 others are typing…')
  })

  it('merges by id and marks a deletion in place', () => {
    const a = mergeHomeChatMessages([msg('2'), msg('1')], [msg('3'), msg('2', { body: 'edited' })])
    expect(a.map((m) => [m.id, m.body])).toEqual([['1', 'm1'], ['2', 'edited'], ['3', 'm3']])
    expect(markDeleted(a, '2')[1]).toMatchObject({ deleted: true, body: '' })
  })

  it('this window keeps the newest messages of the last chats it drew', () => {
    resetHomeChatViewCacheForTests()
    rememberHomeChat('h', '9', Array.from({ length: 150 }, (_, i) => msg(String(i + 1))))
    expect(cachedHomeChat('h', '9')?.length).toBe(100)
    for (let i = 0; i < VIEW_CACHE_CHATS + 1; i++) rememberHomeChat('h', `c${i}`, [msg('1')])
    expect(cachedHomeChat('h', '9')).toBeNull()
  })
})

describe('people (§6)', () => {
  it('the picker takes people whose chat is set up, never agents or App users', () => {
    expect(pickablePerson(person('k2:connect:a'))).toBe(true)
    expect(pickablePerson(person('k2:owner'))).toBe(true)
    expect(pickablePerson(person('k2:connect:a', { ready: false }))).toBe(false)
    expect(pickablePerson(person('k2:workspace:5d1c'))).toBe(false)
    expect(pickablePerson(person('k2:principal:app-1'))).toBe(false)
  })

  it('“same person?” matches a username or a full name; the owner only by name', () => {
    expect(mightBeSamePerson(person('k2:connect:a', { username: 'alex' }), person('k2:connect:b', { username: 'Alex' }))).toBe(true)
    expect(mightBeSamePerson(person('k2:owner', { username: 'owner', displayName: 'Rae Long' }), person('k2:owner', { username: 'owner', displayName: 'Sam Moe' }))).toBe(false)
    expect(mightBeSamePerson(person('k2:owner', { username: 'owner', displayName: 'Rae Long' }), person('k2:connect:x', { username: 'rae', displayName: 'Rae Long' }))).toBe(true)
    const m = matchesElsewhere(person('k2:connect:a', { username: 'alex' }), 'box-a', { 'box-a': [person('k2:connect:q', { username: 'alex' })], 'box-b': [person('k2:connect:z', { username: 'alex' })] })
    expect(m.map((x) => x.hostKey)).toEqual(['box-b'])
  })
})

describe('what a role may do in a channel (§13)', () => {
  const base = { callerExt: 'k2:connect:me', creator: 'k2:connect:other', member: true, open: false, archived: false }
  it('a Member (and a Voice & Chat only login) adds people and leaves; renames only their own', () => {
    expect(channelPerms({ ...base, admin: false })).toEqual({ addPeople: true, removeOthers: false, leave: true, rename: false, remove: false })
    expect(channelPerms({ ...base, admin: false, creator: 'k2:connect:me' })).toMatchObject({ rename: true, remove: true, removeOthers: false })
  })
  it('an Owner or Admin removes others in an invite-only channel and renames any', () => {
    expect(channelPerms({ ...base, admin: true })).toEqual({ addPeople: true, removeOthers: true, leave: true, rename: true, remove: true })
    expect(channelPerms({ ...base, admin: true, open: true }).removeOthers).toBe(false)
  })
  it('an archived channel is read-only', () => {
    expect(channelPerms({ ...base, admin: true, archived: true })).toMatchObject({ addPeople: false, removeOthers: false, rename: false, remove: false })
  })
  it('who counts as Owner or Admin: the owner, or a people list with roles', () => {
    expect(callerIsAdmin(parseHomeChatPeople({ me: { ext: 'k2:owner' }, people: [] }))).toBe(true)
    expect(callerIsAdmin(parseHomeChatPeople({ me: { ext: 'k2:connect:me' }, people: [{ ext: 'k2:owner', username: 'owner' }] }))).toBe(false)
    expect(callerIsAdmin(parseHomeChatPeople({ me: { ext: 'k2:connect:me' }, people: [{ ext: 'k2:owner', username: 'owner', role: 'owner' }] }))).toBe(true)
  })
})
