// `me.onAccessChanged` (0.46.8) against the fake gateway: a read on every open,
// coalesced reads, a dead pass, a rotated pass, a 429. Every test fails loudly.

import { afterEach, describe, expect, it } from 'vitest'

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { K2App, K2Error, fromEnv, type AccessChange } from '../src/index.ts'
import { startFake, type Answer, type FakeGateway, type Seen } from './fake-gateway.ts'

const PASS = 'k2skn_access_test_pass'

function meBody(rooms: string[]): unknown {
  return {
    ok: true,
    kind: 'guest',
    username: 'dana',
    app: 'host',
    principalId: 'p1',
    rooms: rooms.map((r) => ({ room: r, handle: r, caps: ['thread:read'] })),
    agents: [],
    chat: false,
    features: ['live-access-v1'],
    catalogVersion: 45,
    serverId: 's',
    publicHost: 'h',
    instanceId: 'i',
  }
}

async function until(what: string, ok: () => boolean, ms = 3000): Promise<void> {
  const end = Date.now() + ms
  while (!ok()) {
    if (Date.now() > end) throw new Error(`timed out waiting for: ${what}`)
    await new Promise((r) => setTimeout(r, 5))
  }
}

function meCalls(gw: FakeGateway): Seen[] {
  return gw.seen.filter((s) => s.url.startsWith('/cli/skin/me') && !s.url.startsWith('/cli/skin/me/events'))
}

describe('me.onAccessChanged', () => {
  let gw: FakeGateway
  afterEach(async () => {
    await gw.stop()
  })

  function app(): K2App {
    return new K2App({ gatewayUrl: gw.url })
  }

  it('reads on the first open and after each frame; the event carries no answer', async () => {
    let rooms = ['a']
    gw = await startFake(() => ({ status: 200, body: meBody(rooms) }))
    const seen: AccessChange[] = []
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c))
    await until('the first read', () => seen.length === 1)
    expect(seen[0]?.me?.rooms.map((r) => r.room)).toEqual(['a'])
    expect(gw.sockets[0]?.url).toContain('/cli/skin/me/events?scope=me')
    expect(gw.sockets[0]?.url).toContain(`token=${PASS}`)

    rooms = ['a', 'b']
    gw.sockets[0]!.send({ kind: 'access_changed' })
    await until('the next read', () => seen.length === 2)
    expect(seen[1]?.me?.rooms.map((r) => r.room)).toEqual(['a', 'b'])
    off()
  })

  it('reads again on every reopen, so a change while away is recovered', async () => {
    let rooms = ['a']
    gw = await startFake(() => ({ status: 200, body: meBody(rooms) }))
    const seen: AccessChange[] = []
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c), { onError: (e) => { throw e } })
    await until('the first read', () => seen.length === 1)
    rooms = []
    gw.sockets[0]!.close(1011)
    await until('the reopen', () => gw.sockets.length === 2)
    await until('the read after it', () => seen.length === 2)
    expect(seen[1]?.me?.rooms).toEqual([])
    off()
  })

  it('a burst of frames during a read costs one more read, not one each', async () => {
    let release: (() => void) | null = null
    gw = await startFake(() => ({ status: 200, body: meBody(['a']) }))
    let inFlight = 0
    let calls = 0
    const a = {
      call: async () => {
        calls += 1
        inFlight += 1
        await new Promise<void>((r) => {
          release = r
        })
        inFlight -= 1
        return meBody(['a'])
      },
      subscribe: (_v: string, _args: unknown, onFrame: (f: unknown) => void, _e: unknown, o?: { onOpen?: () => void }) => {
        frame = onFrame
        o?.onOpen?.()
        return () => {}
      },
    }
    let frame: (f: unknown) => void = () => {}
    const { watchAccess } = await import('../src/access.ts')
    const seen: AccessChange[] = []
    const off = watchAccess(a as never, (c) => seen.push(c))
    await until('the first read started', () => inFlight === 1)
    for (let i = 0; i < 50; i += 1) frame({ kind: 'access_changed' })
    release!()
    await until('one more read', () => calls === 2 && inFlight === 1)
    release!()
    await until('both answered', () => seen.length === 2)
    await new Promise((r) => setTimeout(r, 30))
    expect(calls).toBe(2)
    off()
  })

  it('a 4003 with a dead pass ends once, and stops', async () => {
    let dead = false
    gw = await startFake(() => (dead ? { status: 401, body: { error: { code: 'pass_invalid', hint: 'gone' } } } : { status: 200, body: meBody(['a']) }))
    const seen: AccessChange[] = []
    app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c))
    await until('the first read', () => seen.length === 1)
    dead = true
    gw.sockets[0]!.close(4003)
    await until('it ended', () => seen.length === 2)
    expect(seen[1]).toEqual({ me: null, ended: true })
    await new Promise((r) => setTimeout(r, 700))
    expect(gw.sockets.length).toBe(1)
    expect(seen.length).toBe(2)
  })

  it('a 4003 with a live pass (an App pass that rotated) reconnects quietly', async () => {
    gw = await startFake(() => ({ status: 200, body: meBody(['a']) }))
    const seen: AccessChange[] = []
    const errors: K2Error[] = []
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c), { onError: (e) => errors.push(e) })
    await until('the first read', () => seen.length === 1)
    gw.sockets[0]!.close(4003)
    await until('the reopen', () => gw.sockets.length === 2)
    await until('its read', () => seen.length === 2)
    expect(seen.every((c) => !c.ended)).toBe(true)
    expect(errors).toEqual([])
    off()
  })

  it('a close code lost on the way: a refused re-dial and a 401 still end it (the pass died while away)', async () => {
    let dead = false
    gw = await startFake(() => (dead ? { status: 401, body: { error: { code: 'pass_invalid', hint: 'gone' } } } : { status: 200, body: meBody(['a']) }))
    const seen: AccessChange[] = []
    app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c))
    await until('the first read', () => seen.length === 1)
    dead = true
    gw.onUpgrade = () => 401
    gw.sockets[0]!.close(1011)
    await until('it ended', () => seen.length === 2, 5000)
    expect(seen[1]).toEqual({ me: null, ended: true })
  })

  it('a read refused 429 waits as long as it says', async () => {
    let limited = true
    const answer = (): Answer => (limited ? { status: 429, body: { error: { code: 'rate_limited', hint: 'slow' } }, headers: { 'Retry-After': '1' } } : { status: 200, body: meBody(['a']) })
    gw = await startFake(answer)
    const seen: AccessChange[] = []
    const errors: K2Error[] = []
    const t0 = Date.now()
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c), { onError: (e) => errors.push(e) })
    await until('the refused read', () => meCalls(gw).length === 1)
    limited = false
    await until('the retry', () => seen.length === 1, 4000)
    expect(Date.now() - t0).toBeGreaterThanOrEqual(900)
    expect(errors).toEqual([])
    off()
  })

  it('a server with no me.events: says why, and still gives what is true now', async () => {
    gw = await startFake(() => ({ status: 200, body: meBody(['a']) }))
    gw.onUpgrade = () => 404
    const seen: AccessChange[] = []
    const errors: K2Error[] = []
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c), { onError: (e) => errors.push(e) })
    await until('the one read', () => seen.length === 1)
    expect(errors.map((e) => e.code)).toEqual(['failed'])
    off()
  })

  it('a failed read goes to onError and it keeps listening', async () => {
    let broken = true
    gw = await startFake(() => (broken ? { status: 500, body: { error: 'boom' } } : { status: 200, body: meBody(['a']) }))
    const seen: AccessChange[] = []
    const errors: K2Error[] = []
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c), { onError: (e) => errors.push(e) })
    await until('the failed read', () => errors.length === 1)
    expect(seen).toEqual([])
    broken = false
    gw.sockets[0]!.send({ kind: 'access_changed' })
    await until('the next read', () => seen.length === 1)
    off()
  })

  it('nothing is called after off', async () => {
    gw = await startFake(() => ({ status: 200, body: meBody(['a']) }))
    const seen: AccessChange[] = []
    const off = app().asGuest(PASS).me.onAccessChanged((c) => seen.push(c))
    await until('the first read', () => seen.length === 1)
    off()
    gw.sockets[0]!.send({ kind: 'access_changed' })
    await new Promise((r) => setTimeout(r, 100))
    expect(seen.length).toBe(1)
  })
})

describe('me.onGuestAccessChanged', () => {
  let gw: FakeGateway
  afterEach(async () => {
    await gw.stop()
  })

  it('names a guest, with the frame scope=guests sends', async () => {
    gw = await startFake(() => ({ status: 200, body: meBody([]) }))
    const dir = mkdtempSync(join(tmpdir(), 'k2-sdk-access-'))
    writeFileSync(join(dir, 'pass'), 'k2skn_the_apps_own\n')
    const k2 = fromEnv({ K2_GATEWAY_URL: gw.url, K2_PASS_FILE: join(dir, 'pass') })
    const named: string[] = []
    let opens = 0
    const off = k2.asApp().me.onGuestAccessChanged((g) => named.push(g.principalId), { onOpen: () => (opens += 1) })
    await until('open', () => opens === 1)
    expect(gw.sockets[0]?.url).toContain('scope=guests')
    gw.sockets[0]!.send({ kind: 'access_changed', principalId: 'p9' })
    await until('the frame', () => named.length === 1)
    expect(named).toEqual(['p9'])
    off()
    rmSync(dir, { recursive: true, force: true })
  })
})
