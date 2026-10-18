// The server SDK against a fake gateway (runtime S23): env, actors (guest
// by default, App written out), the wire (names, query vs JSON),
// projection onto the catalog's closed shapes, pass rotation, refusals,
// the App key, and sockets. Every test fails loudly; no skips.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { APP_VERBS, K2Error, fromEnv, type K2App } from '../src/index.ts'
import { startFake, until, type Answer, type FakeGateway, type Seen } from './fake-gateway.ts'

const GUEST = 'k2skn_guestAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
const APP1 = 'k2skn_appPass1111111111111111111111111111111111'
const APP2 = 'k2skn_appPass2222222222222222222222222222222222'

let dir = ''
let gw: FakeGateway
let answer: (s: Seen) => Answer

function client(env: Record<string, string> = {}): K2App {
  return fromEnv({
    K2_GATEWAY_URL: gw.url,
    K2_PASS_FILE: join(dir, 'pass'),
    K2_APP_ID: 'svc-jobs-1',
    K2_APP_NAME: 'jobs',
    K2_SECRETS_DIR: join(dir, 'secrets'),
    ...env,
  })
}

const bearer = (s: Seen): string | undefined => s.headers.authorization?.replace(/^Bearer /, '')

beforeEach(async () => {
  dir = mkdtempSync(join(tmpdir(), 'k2-sdk-'))
  writeFileSync(join(dir, 'pass'), `${APP1}\n`)
  answer = () => ({ status: 200, body: { ok: true } })
  gw = await startFake((s) => answer(s))
})

afterEach(async () => {
  await gw.stop()
  rmSync(dir, { recursive: true, force: true })
})

describe('fromEnv', () => {
  it('reads the gateway, the pass file and the App identity', () => {
    const k2 = client()
    expect(k2.gatewayUrl).toBe(gw.url)
    expect(k2.appId).toBe('svc-jobs-1')
    expect(k2.appName).toBe('jobs')
  })

  it('falls back to K2_HOST/K2_PORT, and refuses with no address at all', () => {
    const k2 = fromEnv({ K2_HOST: '127.0.0.1', K2_PORT: '5555' })
    expect(k2.gatewayUrl).toBe('http://127.0.0.1:5555')
    expect(() => fromEnv({})).toThrowError(/K2_GATEWAY_URL is not set/)
    expect(() => fromEnv({ K2_GATEWAY_URL: 'not a url' })).toThrowError(/gatewayUrl must be/)
  })
})

describe('actors: the guest by default, the App written out', () => {
  it('a call with no guest bound fails guest_required and sends nothing', async () => {
    const k2 = client()
    const err = await k2.thread.page({ addr: 'sales' }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(K2Error)
    expect((err as K2Error).code).toBe('guest_required')
    expect(gw.seen).toHaveLength(0)
  })

  it('withGuest makes the guest the default for every call inside it (async too)', async () => {
    const k2 = client()
    await k2.withGuest(GUEST, async () => {
      await k2.thread.page({ addr: 'sales' })
      await new Promise((r) => setTimeout(r, 5))
      await k2.me.get()
    })
    expect(gw.seen.map(bearer)).toEqual([GUEST, GUEST])
    expect(k2.currentGuest()).toBeNull()
  })

  it('asApp presents the App pass; asGuest the named guest', async () => {
    const k2 = client()
    await k2.asApp().thread.page({ addr: 'sales' })
    await k2.asGuest(GUEST).thread.page({ addr: 'sales' })
    expect(gw.seen.map(bearer)).toEqual([APP1, GUEST])
    expect(() => k2.asGuest('owner-token')).toThrowError(/guest session pass/)
  })

  it('asApp without a pass file fails no_app_pass', async () => {
    const k2 = client({ K2_PASS_FILE: join(dir, 'none') })
    const err = await k2.asApp().me.get().catch((e: unknown) => e)
    expect((err as K2Error).code).toBe('no_app_pass')
    expect(gw.seen).toHaveLength(0)
  })

  it('middleware binds the request guest from Bearer or the cookie', async () => {
    const k2 = client()
    const mw = k2.middleware()
    await new Promise<void>((done) => mw({ headers: { authorization: `Bearer ${GUEST}` } }, {}, () => void k2.me.get().then(() => done())))
    await new Promise<void>((done) => mw({ headers: { cookie: `a=1; k2_skin_session=${GUEST}` } }, {}, () => void k2.me.get().then(() => done())))
    expect(gw.seen.map(bearer)).toEqual([GUEST, GUEST])
    expect(k2.guestFrom({ headers: { authorization: 'Bearer owner' } })).toBeNull()
  })
})

describe('the wire', () => {
  it('a GET sends every argument in the query under its wire name', async () => {
    const k2 = client()
    await k2.asGuest(GUEST).files.list({ workspace: 'sales', relPath: 'docs', showHidden: true })
    await k2.asGuest(GUEST).thread.latest({ addrs: ['sales', 'ops'] })
    const [a, b] = gw.seen
    expect(a?.method).toBe('GET')
    const u = new URL(a?.url ?? '', gw.url)
    expect(u.pathname).toBe('/cli/fs/read-dir')
    expect(Object.fromEntries(u.searchParams)).toEqual({ workspace: 'sales', path: 'docs', show_hidden: 'true' })
    expect(new URL(b?.url ?? '', gw.url).searchParams.get('addrs')).toBe('sales,ops')
  })

  it('a POST sends JSON under the wire names, with the content type', async () => {
    const k2 = client()
    await k2.asGuest(GUEST).files.uploadChunk({ workspace: 'sales', dir: 'in', filename: 'a.bin', uploadId: 'u1', offset: 0, base64: 'AA==', isLast: true })
    const s = gw.seen[0]
    expect(s?.method).toBe('POST')
    expect(s?.url).toBe('/cli/fs/upload-chunk')
    expect(s?.headers['content-type']).toBe('application/json')
    expect(JSON.parse(s?.body ?? '')).toEqual({ workspace: 'sales', dir: 'in', filename: 'a.bin', upload_id: 'u1', offset: 0, base64: 'AA==', is_last: true })
  })

  it('answers are projected onto the closed shape: banned fields dropped, path renamed', async () => {
    const k2 = client()
    answer = (s) =>
      s.url.startsWith('/cli/thread?')
        ? {
            status: 200,
            body: {
              ok: true, collection: 'thread', addr: 'sales', conversation_id: 'conv-1', has_more: false, pastAddresses: [],
              items: [{ collection: 'thread', seq: 3, id: 'd1', conversation_id: 'conv-1', doc: { id: 'd1', kind: 'text', from: 'dana', created_at: 1, body: 'hi', inject: 'x' } }],
            },
          }
        : s.url.startsWith('/cli/fs/read-dir')
          ? { status: 200, body: [{ name: 'a.txt', path: 'docs/a.txt', isDirectory: false, size: 3, modifiedAt: 5, hostPath: '/home/k2/ws/docs/a.txt' }] }
          : { status: 200, body: { ok: true, table: 'jobs', rows: [{ id: 1, path: '/kept/as/data', nested: { sessionId: 'mine' } }], sessionId: 'host' } }
    const g = k2.asGuest(GUEST)
    const page = await g.thread.page({ addr: 'sales' })
    expect(page).toEqual({
      ok: true, collection: 'thread', addr: 'sales', conversation: 'conv-1', has_more: false, pastAddresses: [],
      items: [{ collection: 'thread', seq: 3, id: 'd1', doc: { id: 'd1', kind: 'text', from: 'dana', created_at: 1, body: 'hi' } }],
    })
    expect(JSON.stringify(page)).not.toContain('conversation_id')
    const entries = await g.files.list({ workspace: 'sales', relPath: 'docs' })
    expect(entries).toEqual([{ name: 'a.txt', relPath: 'docs/a.txt', isDirectory: false, size: 3, modifiedAt: 5 }])
    const rows = await g.db.rows({ workspace: 'sales', table: 'jobs' })
    expect(rows).toEqual({ ok: true, table: 'jobs', rows: [{ id: 1, path: '/kept/as/data', nested: { sessionId: 'mine' } }] })
  })

  it('files.stat (room-scope R3): GET /cli/fs/stat with the room-relative path, answer projected', async () => {
    const k2 = client()
    answer = () => ({
      status: 200,
      body: { name: 'plan.pdf', path: 'jobs/J1/plan.pdf', kind: 'file', size: 81_000_000, modifiedAt: 1.7e12, ext: 'pdf', mime: 'application/pdf', hostPath: '/home/k2/ws/jobs/J1/plan.pdf' },
    })
    const st = await k2.asGuest(GUEST).files.stat({ workspace: 'docs', relPath: 'jobs/J1/plan.pdf' })
    const s = gw.seen[0]
    expect(s?.method).toBe('GET')
    const u = new URL(s?.url ?? '', gw.url)
    expect(u.pathname).toBe('/cli/fs/stat')
    expect(Object.fromEntries(u.searchParams)).toEqual({ workspace: 'docs', path: 'jobs/J1/plan.pdf' })
    expect(st).toEqual({ name: 'plan.pdf', relPath: 'jobs/J1/plan.pdf', kind: 'file', size: 81_000_000, modifiedAt: 1.7e12, ext: 'pdf', mime: 'application/pdf' })
    // A folder outside the login's scope is the missing-cap 403.
    answer = () => ({ status: 403, body: { error: 'missing capability files:read' } })
    const err = (await k2.asGuest(GUEST).files.stat({ workspace: 'docs', relPath: 'jobs/J2/x.pdf' }).catch((e: unknown) => e)) as K2Error
    expect(err).toBeInstanceOf(K2Error)
    expect(err.code).toBe('cap_not_granted')
  })

  it('a browser-only verb and an unknown verb never reach K2', async () => {
    const k2 = client()
    const g = k2.asGuest(GUEST) as unknown as { call: (v: string, a?: unknown) => Promise<unknown> }
    expect(((await g.call('password.change', { oldPassword: 'a', password: 'b' }).catch((e: unknown) => e)) as K2Error).code).toBe('not_on_link')
    expect(((await g.call('nope.nope').catch((e: unknown) => e)) as K2Error).code).toBe('unknown_verb')
    expect(gw.seen).toHaveLength(0)
  })

  it('every server verb has a named helper on the default actor and on asApp()', () => {
    const k2 = client() as unknown as Record<string, Record<string, unknown>>
    const app = client().asApp() as unknown as Record<string, Record<string, unknown>>
    for (const [verb, row] of Object.entries(APP_VERBS)) {
      const [surface, action] = verb.split('.') as [string, string]
      if (row.door === 'helper') {
        expect(k2[surface]?.[action], verb).toBeUndefined()
        continue
      }
      expect(typeof k2[surface]?.[action], verb).toBe('function')
      expect(typeof app[surface]?.[action], verb).toBe('function')
    }
  })
})

describe('the App pass rotates', () => {
  it('a rewritten pass file is used on the next call', async () => {
    const k2 = client()
    await k2.asApp().me.get()
    writeFileSync(join(dir, 'pass'), `${APP2}\n`)
    await k2.asApp().me.get()
    expect(gw.seen.map(bearer)).toEqual([APP1, APP2])
  })

  it('pass_invalid reads the file again and retries once', async () => {
    const k2 = client()
    await k2.asApp().me.get() // caches APP1
    let rotated = false
    answer = (s) => {
      if (bearer(s) === APP1) {
        if (!rotated) {
          rotated = true
          // K2 rotated the file while the old pass expired.
          writeFileSync(join(dir, 'pass'), `${APP2}\n`)
        }
        return { status: 401, body: { error: { code: 'pass_invalid', hint: 'read K2_PASS_FILE again' } } }
      }
      return { status: 200, body: { ok: true } }
    }
    await k2.asApp().me.get()
    expect(gw.seen.map(bearer).slice(1)).toEqual([APP1, APP2])
  })

  it('a guest pass is never retried', async () => {
    const k2 = client()
    answer = () => ({ status: 401, body: { error: 'invalid or revoked skin token' } })
    const err = (await k2.asGuest(GUEST).me.get().catch((e: unknown) => e)) as K2Error
    expect(err.code).toBe('pass_invalid')
    expect(gw.seen).toHaveLength(1)
  })
})

describe('refusals become one K2Error', () => {
  it('maps every shape K2 answers in', async () => {
    const k2 = client()
    const g = k2.asGuest(GUEST)
    const cases: Array<[Answer, string, Partial<K2Error>]> = [
      [{ status: 403, body: { error: 'missing capability thread:read' } }, 'cap_not_granted', { cap: 'thread:read', status: 403 }],
      [{ status: 403, body: { ok: false, error: { code: 'skin_room', hint: 'this pass cannot use that agent' } } }, 'skin_room', {}],
      [{ status: 404, body: { error: { code: 'not_on_link', hint: 'x', app: 'jobs' } } }, 'not_on_link', {}],
      [{ status: 429, body: { error: 'rate_limited', retryAfterSecs: 30 }, headers: { 'retry-after': '30' } }, 'rate_limited', { retryAfterSecs: 30 }],
      [{ status: 404, body: { error: 'no_such_heartbeat' } }, 'not_found', {}],
      [{ status: 500, body: 'boom' }, 'failed', {}],
    ]
    for (const [a, code, more] of cases) {
      answer = () => a
      const err = (await g.thread.page({ addr: 'sales' }).catch((e: unknown) => e)) as K2Error
      expect(err, code).toBeInstanceOf(K2Error)
      expect(err.code, JSON.stringify(a)).toBe(code)
      expect(err.verb).toBe('thread.page')
      for (const [k, v] of Object.entries(more)) expect((err as unknown as Record<string, unknown>)[k], k).toBe(v)
    }
  })
})

describe('login (must-not-break: the BFF logs its guests in)', () => {
  it('names this App, opens the link with the App pass, and renames token to pass', async () => {
    const k2 = client()
    answer = () => ({
      status: 200,
      body: { ok: true, token: GUEST, username: 'dana', principalId: 'p1', rooms: ['r1'], roomHandles: ['sales'], caps: ['thread:read'], role: null, roomAccess: [{ handle: 'sales', caps: ['thread:read'] }], app: 'svc-jobs-1' },
    })
    const s = await k2.login({ username: 'dana', password: 'pw' })
    expect(s.pass).toBe(GUEST)
    expect(s).not.toHaveProperty('token')
    expect(gw.seen[0]?.url).toBe('/cli/skin/login')
    expect(bearer(gw.seen[0] as Seen)).toBe(APP1)
    expect(JSON.parse(gw.seen[0]?.body ?? '')).toEqual({ username: 'dana', password: 'pw', app: 'svc-jobs-1' })
  })

  it('a --cmd service (no pass file) logs in with no credential; 423 carries retryAfterSecs', async () => {
    const k2 = client({ K2_PASS_FILE: join(dir, 'none') })
    answer = () => ({ status: 423, body: { error: 'locked', retryAfterSecs: 900 }, headers: { 'retry-after': '900' } })
    const err = (await k2.login({ username: 'dana', password: 'bad' }).catch((e: unknown) => e)) as K2Error
    expect(err.code).toBe('locked')
    expect(err.retryAfterSecs).toBe(900)
    expect(gw.seen[0]?.headers.authorization).toBeUndefined()
  })
})

describe('users and rooms: the App key', () => {
  it('calls /v1/apps/<K2_APP_ID>/… with the key file, never a pass', async () => {
    const k2 = client()
    const err = (await k2.users.list().catch((e: unknown) => e)) as K2Error
    expect(err.code).toBe('no_app_key')
    expect(gw.seen).toHaveLength(0)
    const { mkdirSync } = await import('node:fs')
    mkdirSync(join(dir, 'secrets'))
    writeFileSync(join(dir, 'secrets', 'K2_APP_KEY'), 'k2sk_appkey\n')
    answer = () => ({ status: 200, body: { ok: true, username: 'dana', app: 'svc-jobs-1', rooms: ['r1'], roomHandles: ['sales'] } })
    await k2.rooms.set({ username: 'dana', rooms: ['sales'] })
    expect(gw.seen[0]?.url).toBe('/v1/apps/svc-jobs-1/users/rooms')
    expect(bearer(gw.seen[0] as Seen)).toBe('k2sk_appkey')
    expect(JSON.parse(gw.seen[0]?.body ?? '')).toEqual({ username: 'dana', rooms: ['sales'] })
    answer = () => ({ status: 404, body: { error: { code: 'not_found', hint: 'not found' } } })
    expect(((await k2.users.list().catch((e: unknown) => e)) as K2Error).code).toBe('not_found')
  })
})

describe('sockets', () => {
  it('delivers projected frames as the guest, and unsubscribe closes it', async () => {
    const k2 = client()
    const frames: unknown[] = []
    const off = k2.asGuest(GUEST).thread.events({ conversation: 'conv-1' }, (f) => frames.push(f))
    await until('the socket', () => gw.sockets.length === 1)
    const conn = gw.sockets[0]!
    const u = new URL(conn.url, gw.url)
    expect(u.pathname).toBe('/cli/overlay/events')
    expect(u.searchParams.get('conversation')).toBe('conv-1')
    expect(u.searchParams.get('token')).toBe(GUEST)
    conn.send({ collection: 'thread', seq: 4, id: 'd4', doc: { id: 'd4', kind: 'text', from: 'agent', created_at: 2, inject: 'x' } })
    await until('a frame', () => frames.length === 1)
    expect(frames[0]).toEqual({ collection: 'thread', seq: 4, id: 'd4', doc: { id: 'd4', kind: 'text', from: 'agent', created_at: 2 } })
    off()
    await conn.closedByClient
  })

  it('a socket opened inside withGuest keeps that guest; none bound fails guest_required', async () => {
    const k2 = client()
    expect(() => k2.files.events({ workspace: 'sales' }, () => {})).toThrowError(/guest_required/)
    const off = k2.withGuest(GUEST, () => k2.files.events({ workspace: 'sales' }, () => {}))
    await until('the socket', () => gw.sockets.length === 1)
    expect(new URL(gw.sockets[0]!.url, gw.url).searchParams.get('token')).toBe(GUEST)
    off()
  })

  it('a refused socket ends with one failed error and no retry', async () => {
    const k2 = client()
    gw.onUpgrade = () => 403
    const errors: K2Error[] = []
    k2.asGuest(GUEST).activity.events({ workspace: 'ops' }, () => {}, (e) => errors.push(e))
    await until('the refusal', () => errors.length === 1)
    await new Promise((r) => setTimeout(r, 800))
    expect(errors.map((e) => e.code)).toEqual(['failed'])
    expect(gw.sockets).toHaveLength(0)
  })

  it('a 4008 lag close says resync and reconnects', async () => {
    const k2 = client()
    const errors: K2Error[] = []
    const off = k2.asApp().activity.events({ workspace: 'ops' }, () => {}, (e) => errors.push(e))
    await until('the socket', () => gw.sockets.length === 1)
    gw.sockets[0]!.close(4008)
    await until('the resync', () => errors.length === 1)
    expect(errors[0]?.code).toBe('resync')
    expect(errors[0]?.hint).toMatch(/4008.*refetch with activity\.snapshot/)
    await until('the reconnect', () => gw.sockets.length === 2, 5000)
    off()
  })
})
