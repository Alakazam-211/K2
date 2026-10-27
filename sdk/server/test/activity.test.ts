// thread.watchActivity (0.46.5 activity SDK) against the fake gateway: the
// socket first, ONE batched read on every open (thread.activityRooms),
// frames by room, the daemon's activity_reset, the {addr} form, the
// onOpen signal and the refetch hint. Every test fails loudly.

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { APP_VERBS, fromEnv, type ActivityState, type K2App } from '../src/index.ts'
import { refetchHint } from '../src/socket.ts'
import { startFake, until, type Answer, type FakeGateway, type Seen } from './fake-gateway.ts'

const GUEST = 'k2skn_guestAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
const fixture = JSON.parse(readFileSync(resolve(__dirname, '../../../tests/fixtures/activity/guest-turn-parity.json'), 'utf8')) as {
  cases: Array<{ name: string; guest: Record<string, unknown> }>
}
const guest = (part: string): Record<string, unknown> => {
  const c = fixture.cases.find((x) => x.name.includes(part))
  if (!c) throw new Error(part)
  return c.guest
}

let dir = ''
let gw: FakeGateway
let answer: (s: Seen) => Answer

function client(): K2App {
  return fromEnv({ K2_GATEWAY_URL: gw.url, K2_PASS_FILE: join(dir, 'pass'), K2_APP_ID: 'svc-jobs-1' })
}

beforeEach(async () => {
  dir = mkdtempSync(join(tmpdir(), 'k2-sdk-act-'))
  writeFileSync(join(dir, 'pass'), 'k2skn_appPass1111111111111111111111111111111111\n')
  answer = () => ({ status: 200, body: { ok: true } })
  gw = await startFake((s) => answer(s))
})

afterEach(async () => {
  await gw.stop()
  rmSync(dir, { recursive: true, force: true })
})

const reads = () => gw.seen.filter((s) => s.url.startsWith('/cli/thread/activity'))

describe('thread.watchActivity', () => {
  it('{rooms}: socket first; on every open ONE batched read; frames by room; a reset replaces', async () => {
    answer = (s) =>
      s.url.startsWith('/cli/thread/activity/rooms')
        ? { status: 200, body: { ok: true, turns: { r1: guest('tool step'), r2: null }, unknown: [] } }
        : { status: 404, body: { error: { code: 'not_on_link' } } }
    const k2 = client()
    let last: Map<string, ActivityState> | undefined
    const off = k2.asGuest(GUEST).thread.watchActivity({ rooms: 'all' }, (m) => (last = m))
    await until('the socket', () => gw.sockets.length === 1)
    const u = new URL(gw.sockets[0]!.url, gw.url)
    expect([u.pathname, u.searchParams.get('rooms'), u.searchParams.get('ear')]).toEqual(['/cli/thread/stream', 'all', '1'])
    expect(reads(), 'no read before the socket says it is open').toHaveLength(0)
    gw.sockets[0]!.send({ kind: 'inbox_subscribed', rooms: ['r1', 'r2'], resolved: { r1: 'c1', r2: 'c2' } })
    await until('the batched read', () => reads().length === 1)
    expect(reads()[0]!.url).toBe('/cli/thread/activity/rooms?rooms=all')
    expect(reads()[0]!.headers.authorization).toBe(`Bearer ${GUEST}`)
    await until('the snapshot', () => last?.get('r1')?.turn?.turnId === 'msg-1')
    expect(last?.get('r1')).toMatchObject({ live: true, source: 'thread', fresh: 'live' })
    expect(last?.get('r1')?.turn?.step.kind, 'schema 2 reaches App code').toBe('cmd')
    expect(last?.get('r2')?.turn).toBeNull()
    // A frame moves the turn on.
    gw.sockets[0]!.send({ kind: 'thread_activity', room: 'r2', conversation: 'c2', conversation_id: 'c2', id: 'msg-2', activity: guest('permission') })
    await until('the frame', () => last?.get('r2')?.turn?.state === 'needs-you')
    // The daemon fell behind: its reset carries the turns now.
    gw.sockets[0]!.send({ kind: 'activity_reset', turns: { r1: null, r2: guest('permission') } })
    await until('the reset', () => last?.get('r1')?.turn === null)
    expect(reads(), 'a reset with turns needs no read').toHaveLength(1)
    off()
    await gw.sockets[0]!.closedByClient
  })

  it('{addr}: thread.activity names the pinned Chat; frames of other rooms are not this agent’s', async () => {
    answer = (s) =>
      s.url.startsWith('/cli/thread/activity?')
        ? { status: 200, body: { ok: true, addr: 'sales', conversation_id: 'c-sales', turn: null } }
        : { status: 404, body: { error: { code: 'not_on_link' } } }
    const k2 = client()
    let last: Map<string, ActivityState> | undefined
    const off = k2.asGuest(GUEST).thread.watchActivity({ addr: 'sales' }, (m) => (last = m))
    await until('the socket', () => gw.sockets.length === 1)
    gw.sockets[0]!.send({ kind: 'inbox_subscribed', rooms: ['r9'], resolved: { r9: 'c-sales' } })
    await until('the read', () => reads().length === 1)
    expect(reads()[0]!.url).toBe('/cli/thread/activity?addr=sales')
    await until('the snapshot', () => last?.has('sales') === true)
    gw.sockets[0]!.send({ kind: 'thread_activity', room: 'r1', conversation: 'c-other', activity: guest('tool step') })
    gw.sockets[0]!.send({ kind: 'thread_activity', room: 'r9', conversation: 'c-sales', activity: guest('monitoring') })
    await until('our frame', () => last?.get('sales')?.turn?.state === 'monitoring')
    expect([...(last?.keys() ?? [])]).toEqual(['sales'])
    off()
  })

  it('a 429 waits retryAfterMs; the state stays catching-up until a read lands', async () => {
    let n = 0
    answer = () => {
      n += 1
      return n === 1
        ? { status: 429, body: { error: 'rate_limited', retryAfterMs: 1200, retryAfterSecs: 2 }, headers: { 'retry-after': '2' } }
        : { status: 200, body: { ok: true, turns: { r1: null }, unknown: [] } }
    }
    const k2 = client()
    let last: Map<string, ActivityState> | undefined
    const off = k2.asGuest(GUEST).thread.watchActivity({ rooms: ['r1'] }, (m) => (last = m))
    await until('the socket', () => gw.sockets.length === 1)
    gw.sockets[0]!.send({ kind: 'inbox_subscribed', rooms: ['r1'], resolved: { r1: 'c1' } })
    await until('the refused read', () => reads().length === 1)
    expect(reads()[0]!.url).toBe('/cli/thread/activity/rooms?rooms=r1')
    await new Promise((r) => setTimeout(r, 600))
    expect(reads(), 'no read before the wait').toHaveLength(1)
    await until('the retry', () => reads().length === 2, 5000)
    await until('caught up', () => last?.get('r1')?.fresh === 'live')
    off()
  })
})

describe('the App socket (review D5, B2)', () => {
  it('subscribe(…, {onOpen}) says when the socket is open, on every reconnect', async () => {
    const k2 = client()
    let opens = 0
    const off = k2.asGuest(GUEST).subscribe('thread.stream', { rooms: 'all' }, () => {}, () => {}, { onOpen: () => (opens += 1) })
    await until('the first open', () => opens === 1)
    gw.sockets[0]!.close(4008)
    await until('the reopen', () => opens === 2, 5000)
    off()
  })

  it('the ear’s resync hint names thread.activity beside thread.page', () => {
    expect(refetchHint(APP_VERBS['thread.stream'])).toBe('thread.page and thread.activity')
    expect(refetchHint(APP_VERBS['activity.events'])).toBe('activity.snapshot')
  })
})
