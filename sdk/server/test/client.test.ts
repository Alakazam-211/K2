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

  it('Thread parity (catalog v16): afterSeq deltas, attachments without a path, catch-up, read cursors, links', async () => {
    const k2 = client()
    const att = { kind: 'file', state: 'ok', path: 'docs/a.png', name: 'a.png', size: 9, mime: 'image/png', modifiedAt: 1.7e12, sha256: 'ab' }
    const item = (seq: number, id: string, attachments: unknown[]) => ({
      collection: 'thread', seq, id, conversation_id: 'conv-1',
      doc: { id, kind: 'text', from: 'dana', created_at: 1, body: 'hi', attachments },
    })
    answer = (s) =>
      s.url.startsWith('/cli/thread?')
        ? {
            status: 200,
            body: {
              ok: true, collection: 'thread', addr: 'sales', conversation_id: 'conv-1', after_seq: 4, has_more: false, pastAddresses: [], rev: 9,
              items: [item(5, 'd5', [att, { kind: 'file', state: 'hidden' }])],
              changed: [item(3, 'd3', [])],
              removed: [{ id: 'd2', seq: 2 }],
            },
          }
        : s.url.startsWith('/cli/thread/catchup')
          ? { status: 200, body: { ok: true, has_more: false, threads: [{ addr: 'sales', conversation_id: 'conv-1', after_seq: 4, items: [], has_more: false, changed: [], removed: [] }, { addr: 'ops', error: { code: 'skin_room', hint: 'x' } }], groups: [] } }
          : s.url.startsWith('/cli/thread/read')
            ? { status: 200, body: { ok: true, changed: true, kind: 'thread', id: 'conv-1', seq: 5, lastReadSeq: 5, lastSeq: 5, unread: 0, waiting: 0, muted: false, addr: 'sales' } }
            : s.url.startsWith('/cli/fs/resolve-links')
              ? { status: 200, body: { links: [{ input: 'docs/a.png', linkable: true, kind: 'file', rel: 'docs/a.png' }, { input: 'x', linkable: false }] } }
              : { status: 200, body: { ok: true, kind: 'image', mime: 'image/jpeg', width: 256, height: 128, base64: 'AA==', state: 'ok', name: 'a.png', size: 9, px: 256 } }
    const g = k2.asGuest(GUEST)

    const delta = await g.thread.page({ addr: 'sales', afterSeq: 4 })
    const u = new URL(gw.seen[0]?.url ?? '', gw.url)
    expect(u.searchParams.get('after_seq')).toBe('4')
    expect(delta).toEqual({
      ok: true, collection: 'thread', addr: 'sales', conversation: 'conv-1', after_seq: 4, has_more: false, pastAddresses: [],
      items: [{ collection: 'thread', seq: 5, id: 'd5', doc: { id: 'd5', kind: 'text', from: 'dana', created_at: 1, body: 'hi', attachments: [
        { kind: 'file', state: 'ok', name: 'a.png', size: 9, mime: 'image/png', modifiedAt: 1.7e12 },
        { kind: 'file', state: 'hidden' },
      ] } }],
      changed: [{ collection: 'thread', seq: 3, id: 'd3', doc: { id: 'd3', kind: 'text', from: 'dana', created_at: 1, body: 'hi', attachments: [] } }],
      removed: [{ id: 'd2', seq: 2 }],
    })
    expect(JSON.stringify(delta)).not.toContain('docs/a.png')

    const thumb = await g.thread.attachmentGet({ addr: 'sales', id: 'd5', n: 0, mode: 'thumb', px: 256 })
    expect(Object.fromEntries(new URL(gw.seen[1]?.url ?? '', gw.url).searchParams)).toEqual({ addr: 'sales', id: 'd5', n: '0', mode: 'thumb', px: '256' })
    expect(thumb).toEqual({ ok: true, kind: 'image', mime: 'image/jpeg', width: 256, height: 128, base64: 'AA==', state: 'ok', name: 'a.png', size: 9 })

    const cu = await g.thread.catchup({ threads: [{ addr: 'sales', afterSeq: 4 }, { addr: 'ops', newest: true }] })
    expect(gw.seen[2]?.method).toBe('POST')
    expect(JSON.parse(gw.seen[2]?.body ?? '')).toEqual({ threads: [{ addr: 'sales', afterSeq: 4 }, { addr: 'ops', newest: true }] })
    expect(cu.threads[0]).toEqual({ addr: 'sales', conversation: 'conv-1', after_seq: 4, items: [], has_more: false, changed: [], removed: [] })
    expect(cu.threads[1]).toEqual({ addr: 'ops', error: { code: 'skin_room', hint: 'x' } })

    const rd = await g.thread.setRead({ addr: 'sales', seq: 5 })
    expect(JSON.parse(gw.seen[3]?.body ?? '')).toEqual({ addr: 'sales', seq: 5 })
    // catalog v17 (app-read-state-v1): the read state carries addr and muted.
    expect(rd).toEqual({ ok: true, changed: true, id: 'conv-1', addr: 'sales', lastReadSeq: 5, lastSeq: 5, unread: 0, waiting: 0, muted: false })

    const links = await g.files.resolveLinks({ workspace: 'sales', paths: ['docs/a.png', 'x'] })
    expect(links).toEqual({ links: [{ input: 'docs/a.png', linkable: true, kind: 'file', rel: 'docs/a.png' }, { input: 'x', linkable: false }] })

    await g.thread.send({ addr: 'sales', text: '', attachments: [{ relPath: 'docs/a.png', line: 2 }] })
    expect(JSON.parse(gw.seen[5]?.body ?? '')).toEqual({ addr: 'sales', text: '', attachments: [{ relPath: 'docs/a.png', line: 2 }] })
  })

  it('app-read-state-v1 (catalog v17): a guest mutes its own Thread and reads its badgeKey and mutes', async () => {
    const k2 = client()
    answer = (s) =>
      s.url.startsWith('/cli/thread/mute')
        ? { status: 200, body: { ok: true, changed: true, kind: 'thread', id: 'conv-1', seq: 4, lastReadSeq: 4, lastSeq: 5, unread: 1, waiting: 0, muted: true, mutedUntil: 1900000000, addr: 'sales', total: 0, waitingTotal: 0 } }
        : { status: 200, body: { ok: true, total: 0, waitingTotal: 0, badgeKey: 'bk-1', person: 'guest:x', byConversation: [{ kind: 'thread', id: 'conv-1', seq: 4, lastReadSeq: 4, lastSeq: 5, unread: 1, waiting: 0, muted: true, mutedUntil: 1900000000, addr: 'sales' }], muted: [{ id: 'conv-1', addr: 'sales', mutedUntil: 1900000000 }] } }
    const g = k2.asGuest(GUEST)
    const m = await g.thread.mute({ addr: 'sales', muted: true, until: 1900000000 })
    expect(gw.seen[0]?.method).toBe('POST')
    expect(new URL(gw.seen[0]?.url ?? '', gw.url).pathname).toBe('/cli/thread/mute')
    expect(JSON.parse(gw.seen[0]?.body ?? '')).toEqual({ addr: 'sales', muted: true, until: 1900000000 })
    expect(m).toEqual({ ok: true, changed: true, id: 'conv-1', addr: 'sales', lastReadSeq: 4, lastSeq: 5, unread: 1, waiting: 0, muted: true, mutedUntil: 1900000000, total: 0, waitingTotal: 0 })
    const u = await g.thread.unread()
    expect(u).toEqual({
      ok: true, total: 0, waitingTotal: 0, badgeKey: 'bk-1',
      byConversation: [{ id: 'conv-1', addr: 'sales', lastReadSeq: 4, lastSeq: 5, unread: 1, waiting: 0, muted: true, mutedUntil: 1900000000 }],
      muted: [{ id: 'conv-1', addr: 'sales', mutedUntil: 1900000000 }],
    })
    expect(JSON.stringify(u)).not.toContain('guest:x')
  })

  it('a browser-only verb and an unknown verb never reach K2', async () => {
    const k2 = client()
    const g = k2.asGuest(GUEST) as unknown as { call: (v: string, a?: unknown) => Promise<unknown> }
    expect(((await g.call('wallet.list', {}).catch((e: unknown) => e)) as K2Error).code).toBe('not_on_link')
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

describe('room-scope surfaces (0.46.1): room.members, app.self, app.usage', () => {
  it('room.members is a guest GET with its room; app.self/app.usage go as the App with the App pass', async () => {
    const k2 = client()
    answer = (s) => {
      const u = new URL(s.url, gw.url)
      if (u.pathname === '/cli/skin/members') {
        return {
          status: 200,
          body: {
            workspace: 'sales', room: 'p-1', namesHidden: true, admins: 'admins of this box', count: 1, secret: 'x',
            members: [{ username: null, displayName: null, kind: 'guest', role: 'staff', app: 'jobs', caps: ['room:members'], principalId: 'p-9' }],
          },
        }
      }
      if (u.pathname === '/cli/skin/self') {
        return { status: 200, body: { app: 'jobs', workspace: 'sales', liveSeed: 'ab', health: 'running', state: 'running', deployment: 'd-1', deployedAt: 1760000000, path: '/srv' } }
      }
      return {
        status: 200,
        body: {
          app: 'jobs', hours: 24, quotas: { rps: 50, burst: 200, sockets: 64, bodyBytes: 1, uploadBytes: 2, subscriptions: 256 },
          live: { connections: 1, subscriptions: 0 }, totals: { calls: 2, denied: 1, bytesIn: 3, bytesOut: 4 },
          verbs: [{ verb: 'thread.page', route: 'GET /cli/thread', calls: 2, denied: 1 }],
        },
      }
    }
    const m = await k2.asGuest(GUEST).room.members({ workspace: 'sales' })
    expect(m.members[0]?.username).toBeNull()
    expect(m.admins).toBe('admins of this box')
    expect('secret' in m, 'fields the catalog does not name are dropped').toBe(false)
    expect('principalId' in (m.members[0] ?? {}), 'closed at every depth').toBe(false)
    const me = await k2.asApp().app.self()
    expect(me.health).toBe('running')
    expect('path' in me).toBe(false)
    const use = await k2.asApp().app.usage({ hours: 24 })
    expect(use.verbs[0]?.verb).toBe('thread.page')
    const [a, b, c] = gw.seen
    expect(new URL(a?.url ?? '', gw.url).pathname + '?' + new URL(a?.url ?? '', gw.url).searchParams.toString()).toBe('/cli/skin/members?workspace=sales')
    expect(bearer(a as Seen)).toBe(GUEST)
    expect(b?.url).toBe('/cli/skin/self')
    expect(new URL(c?.url ?? '', gw.url).searchParams.get('hours')).toBe('24')
    expect([b, c].map((s) => bearer(s as Seen))).toEqual([APP1, APP1])
    // The App's own data, never a guest's call by accident.
    const err = await k2.app.self().catch((e: unknown) => e)
    expect((err as K2Error).code).toBe('guest_required')
  })

  it('a not_found for another App and not_exposed for a guest come back as K2Error codes', async () => {
    const k2 = client()
    answer = (s) =>
      s.url.startsWith('/cli/skin/self')
        ? { status: 404, body: { ok: false, error: { code: 'not_found', hint: 'no such App' } } }
        : { status: 403, body: { ok: false, error: { code: 'not_exposed', hint: 'never a guest' } } }
    const nf = await k2.asApp().app.self({ app: 'other' }).catch((e: unknown) => e)
    expect((nf as K2Error).code).toBe('not_found')
    const ne = await k2.asGuest(GUEST).app.usage().catch((e: unknown) => e)
    expect((ne as K2Error).code).toBe('not_exposed')
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
    if ('mustChangePassword' in s) throw new Error(`a normal login answered must-change: ${JSON.stringify(s)}`)
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

describe('login: must change password (K2 0.46.6, catalog v37)', () => {
  const TOKEN = 'k2chg_' + 'ab12'.repeat(16)
  const mustChange = (extra: Record<string, unknown> = {}): Answer => ({
    status: 403,
    body: {
      ok: false,
      error: { code: 'password_change_required', hint: 'this person must choose a new password before signing in' },
      mustChangePassword: true,
      changeToken: TOKEN,
      changeExpiresAt: 1_900_000_000,
      changePath: '/cli/skin/password/set',
      changeUrl: `https://box.example/cli/skin/password/set#${TOKEN}`,
      username: 'dana',
      app: 'svc-jobs-1',
      ...extra,
    },
  })

  it('returns mustChangePassword with the change token, never a pass', async () => {
    const k2 = client()
    answer = () => mustChange()
    const s = await k2.login({ username: 'dana', password: 'temp-pw' })
    if (!('mustChangePassword' in s)) throw new Error(`expected must-change, got ${JSON.stringify(s)}`)
    expect(s).toEqual({
      ok: false,
      mustChangePassword: true,
      changeToken: TOKEN,
      changeUrl: `https://box.example/cli/skin/password/set#${TOKEN}`,
      changePath: '/cli/skin/password/set',
      changeExpiresAt: 1_900_000_000,
      username: 'dana',
      app: 'svc-jobs-1',
    })
    expect(s).not.toHaveProperty('pass')
    // The guest pass is never something a caller could bind from it.
    expect(() => k2.asGuest((s as unknown as { pass: string }).pass)).toThrowError(/asGuest takes a guest session pass/)
  })

  it('changeUrl is null when the box has no front door', async () => {
    const k2 = client()
    answer = () => mustChange({ changeUrl: null })
    const s = await k2.login({ username: 'dana', password: 'temp-pw' })
    if (!('mustChangePassword' in s)) throw new Error('expected must-change')
    expect(s.changeUrl).toBeNull()
    expect(s.changePath).toBe('/cli/skin/password/set')
  })

  it('a malformed must-change answer is a thrown error, never a login', async () => {
    const k2 = client()
    for (const bad of [{ changeToken: 'k2skn_notachangetoken' }, { changeToken: 42 }, { mustChangePassword: false }]) {
      answer = () => mustChange(bad)
      const err = (await k2.login({ username: 'dana', password: 'temp-pw' }).catch((e: unknown) => e)) as K2Error
      expect(err).toBeInstanceOf(K2Error)
      expect(err.code).toBe('password_change_required')
    }
  })

  it('an older server SDK (login = the plain guests.login call) fails closed with the code and the hint', async () => {
    // Before 0.46.6, k2.login was exactly the plain guests.login call:
    // a non-2xx is thrown, so an older BFF never sees a session.
    const k2 = client()
    answer = () => mustChange()
    const err = (await k2.asApp().call('guests.login', { username: 'dana', password: 'temp-pw' }).catch((e: unknown) => e)) as K2Error
    expect(err).toBeInstanceOf(K2Error)
    expect(err.code).toBe('password_change_required')
    expect(err.status).toBe(403)
    expect(err.hint).toMatch(/new password/)
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

  it('the room stream asks for the counts-only ear (ear=1); thread.events and other sockets never do', async () => {
    const k2 = client()
    const offs = [
      k2.asGuest(GUEST).thread.events({ conversation: 'c1' }, () => {}),
      k2.asGuest(GUEST).thread.stream({ rooms: 'all' }, () => {}),
      k2.asGuest(GUEST).activity.events({ workspace: 'ops' }, () => {}),
    ]
    await until('three sockets', () => gw.sockets.length === 3)
    const byPath = Object.fromEntries(gw.sockets.map((s) => { const u = new URL(s.url, gw.url); return [u.pathname, u.searchParams] }))
    expect(byPath['/cli/overlay/events']?.get('ear')).toBeNull()
    expect(byPath['/cli/overlay/events']?.get('conversation')).toBe('c1')
    expect(byPath['/cli/thread/stream']?.get('ear')).toBe('1')
    expect(byPath['/cli/activity/events']?.get('ear')).toBeNull()
    for (const off of offs) off()
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

describe('guests.rooms / guests.setRooms (RS-A15: no owner token)', () => {
  const view = {
    ok: true,
    app: 'svc-jobs-1',
    username: 'dana',
    rooms: ['p-acme'],
    roomHandles: ['acme'],
    role: null,
    enabled: true,
    changed: true,
  }

  it('asApp sets a guest’s rooms with the App pass: POST /cli/skin/guests/rooms, JSON body', async () => {
    const k2 = client()
    answer = () => ({ status: 200, body: { ...view, grant: 'dropped by the closed shape' } })
    const v = await k2.asApp().guests.setRooms({ username: 'dana', rooms: ['acme'] })
    expect(v).toEqual(view)
    expect(gw.seen).toHaveLength(1)
    const s = gw.seen[0]!
    expect([s.method, s.url, bearer(s)]).toEqual(['POST', '/cli/skin/guests/rooms', APP1])
    expect(s.headers['content-type']).toBe('application/json')
    expect(JSON.parse(s.body)).toEqual({ username: 'dana', rooms: ['acme'] })
  })

  it('asApp reads one guest: GET with the username in the query', async () => {
    const k2 = client()
    answer = () => ({ status: 200, body: { ...view, changed: false } })
    const v = await k2.asApp().guests.rooms({ username: 'dana' })
    expect(v.changed).toBe(false)
    const s = gw.seen[0]!
    expect(s.method).toBe('GET')
    const u = new URL(s.url, gw.url)
    expect([u.pathname, u.searchParams.get('username'), bearer(s)]).toEqual(['/cli/skin/guests/rooms', 'dana', APP1])
  })

  it('without asApp (no guest bound) it fails guest_required and sends nothing', async () => {
    const k2 = client()
    const err = await k2.guests.setRooms({ username: 'dana', rooms: [] }).catch((e: unknown) => e)
    expect((err as K2Error).code).toBe('guest_required')
    expect(gw.seen).toHaveLength(0)
  })

  it('the refusals are K2Errors with the shared codes', async () => {
    const k2 = client()
    answer = () => ({ status: 403, body: { ok: false, error: { code: 'grant_required', hint: 'ask the owner' } } })
    const err = (await k2.asApp().guests.setRooms({ username: 'dana', rooms: ['acme'] }).catch((e: unknown) => e)) as K2Error
    expect([err.code, err.status]).toEqual(['grant_required', 403])
    answer = () => ({ status: 429, body: { ok: false, error: { code: 'rate_limited', hint: 'slow', retryAfterMs: 900 } }, headers: { 'retry-after': '1' } })
    const limited = (await k2.asApp().guests.rooms({ username: 'dana' }).catch((e: unknown) => e)) as K2Error
    expect(limited.code).toBe('rate_limited')
    expect(APP_VERBS['guests.setRooms'].cap).toBe('guests:rooms')
    expect(APP_VERBS['guests.setRooms'].door).toBe('link')
  })
})
