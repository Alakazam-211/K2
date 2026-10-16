// The must-not-break contract (prd-k2-app-runtime-v1 §14.2 + AR40, S23):
// Dannon's and NSI's routes, driven through the server SDK, against a live
// daemon. The same suite runs two ways:
//
// - from outside (tests/contract/run.ts): the --cmd path Dannon's BFFs use
//   today (K2_GATEWAY_URL = the daemon's loopback port, no App pass);
// - from inside a wall (tests/contract/fixture-app/server.mjs, deployed by
//   tests/linux/apps_e2e_sdk.sh): the App's gateway link, on the guarded
//   and the open path. Linux only.
//
// Every check records ok or the exact failure; the caller fails the run on
// any failed check. Nothing is skipped silently: a check that can't run
// here (no Postgres on this box) is listed under `notRun` with the reason,
// and the runner decides whether that is allowed.

import type { K2App } from '../../sdk/server/src/index.ts'
import { K2Error } from '../../sdk/server/src/index.ts'

export interface Fixture {
  /** A room the guest holds with thread:read/post, files:read/write, store:read/write. */
  room: { handle: string; id: string }
  /** The guest (a login for this App, or the box login on the --cmd path). */
  guest: { username: string; email: string; password: string }
  /** A throwaway guest for the lock check (3 wrong passwords lock it 15 minutes). */
  lockUser: { username: string; email: string; password: string }
  /** A pending choice card in the room's Thread with an option labelled `answer`. */
  card: { id: string; answer: string } | null
  /** A dump table the guest may insert into and read (RLS on k2.skin_uid()); null = no Postgres here. */
  db: { table: string } | null
  /** What a login answers in `app`: a service id, `host`, or '' = this process's K2_APP_ID. */
  expectApp: string
  /** Inside a wall: the identity env K2 sets (AR40). */
  checkEnv: boolean
}

export interface CheckResult {
  name: string
  ok: boolean
  detail: string
}

export interface SuiteResult {
  ok: boolean
  passed: number
  failed: CheckResult[]
  notRun: Array<{ name: string; why: string }>
  results: CheckResult[]
}

class Fail extends Error {}

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Fail(msg)
}

function describeErr(e: unknown): string {
  if (e instanceof K2Error) return `K2Error ${e.code} (HTTP ${e.status}): ${e.hint}`
  if (e instanceof Error) return `${e.name}: ${e.message}`
  return String(e)
}

/** Run the whole suite. `env` is the process env (for the AR40 env check). */
export async function runMustNotBreak(k2: K2App, fx: Fixture, env: Record<string, string | undefined>): Promise<SuiteResult> {
  const wantApp = fx.expectApp || env.K2_APP_ID || ''
  const results: CheckResult[] = []
  const notRun: Array<{ name: string; why: string }> = []
  const check = async (name: string, f: () => Promise<string | void>): Promise<void> => {
    try {
      const detail = await f()
      results.push({ name, ok: true, detail: detail ?? '' })
    } catch (e) {
      results.push({ name, ok: false, detail: describeErr(e) })
    }
  }

  // ── login (§14.2; AR40) ──────────────────────────────────────────
  let pass = ''
  await check('login: POST /cli/skin/login answers the token JSON', async () => {
    const s = await k2.login({ username: fx.guest.username, password: fx.guest.password })
    assert(/^k2skn_/.test(s.pass), `pass ${JSON.stringify(s.pass)}`)
    assert(s.username === fx.guest.username, `username ${s.username}`)
    assert(typeof s.principalId === 'string' && s.principalId.length > 0, 'principalId')
    assert(s.rooms.includes(fx.room.id), `rooms ${JSON.stringify(s.rooms)} lack ${fx.room.id}`)
    assert(s.roomHandles.includes(fx.room.handle), `roomHandles ${JSON.stringify(s.roomHandles)}`)
    assert(s.caps.includes('thread:read') && s.caps.includes('thread:post'), `caps ${JSON.stringify(s.caps)}`)
    const access = s.roomAccess.find((r) => r.handle === fx.room.handle)
    assert(access && access.caps.includes('files:read'), `roomAccess ${JSON.stringify(s.roomAccess)}`)
    assert(s.app === wantApp, `app ${s.app}, want ${wantApp} (the login body's app field)`)
    pass = s.pass
  })
  await check('login: by email (AR40)', async () => {
    const s = await k2.login({ username: fx.guest.email, password: fx.guest.password })
    assert(s.username === fx.guest.username, `email login answered ${s.username}`)
  })
  await check('login: 423 locked with retryAfterSecs and Retry-After, per person (AR40)', async () => {
    for (let i = 0; i < 3; i++) {
      const e = await k2.login({ username: fx.lockUser.username, password: `wrong-${i}` }).catch((x: unknown) => x)
      assert(e instanceof K2Error, `wrong password ${i + 1} answered OK`)
      assert(e.code === 'bad_login' || e.code === 'locked', `wrong password ${i + 1}: ${describeErr(e)}`)
    }
    // The right password, by email: the lock is the person's, not the spelling's.
    const e = await k2.login({ username: fx.lockUser.email, password: fx.lockUser.password }).catch((x: unknown) => x)
    assert(e instanceof K2Error, 'a locked person logged in by email')
    assert(e.code === 'locked' && e.status === 423, `want 423 locked, got ${describeErr(e)}`)
    const body = e.body as { retryAfterSecs?: unknown }
    assert(typeof body.retryAfterSecs === 'number' && body.retryAfterSecs > 0, `retryAfterSecs ${JSON.stringify(e.body)}`)
    assert(typeof e.retryAfterSecs === 'number' && e.retryAfterSecs > 0, 'Retry-After / retryAfterSecs on the error')
  })
  if (!pass) {
    return finish(results, notRun, 'every later check needs the guest login')
  }
  const g = k2.asGuest(pass)

  // ── who am I (R6) ────────────────────────────────────────────────
  await check('me: GET /cli/skin/me names the guest, its rooms and caps, and the features', async () => {
    const me = await g.me.get()
    assert(me.kind === 'guest' && me.username === fx.guest.username, JSON.stringify(me))
    const room = me.rooms.find((r) => r.room === fx.room.id)
    assert(room && room.handle === fx.room.handle && room.caps.includes('thread:post'), JSON.stringify(me.rooms))
    assert(me.features.includes('app-api-v1'), `features ${JSON.stringify(me.features)}`)
  })

  // ── Thread (§14.2) ───────────────────────────────────────────────
  let conversation = ''
  await check('thread: GET /cli/thread', async () => {
    const page = await g.thread.page({ addr: fx.room.handle, limit: 10 })
    assert(page.ok === true && Array.isArray(page.items), JSON.stringify(page).slice(0, 300))
    assert(typeof page.conversation === 'string' && page.conversation.length > 0, 'conversation')
    conversation = page.conversation
  })
  const text = `contract ${Date.now()}`
  await check('thread: WS /cli/overlay/events delivers a post; POST /cli/thread/post', async () => {
    assert(conversation, 'needs the conversation from GET /cli/thread')
    const got = new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => {
        off()
        reject(new Fail('no overlay frame with the post within 15 s'))
      }, 15000)
      const off = g.thread.events(
        { conversation },
        (item) => {
          if (item.doc.body === text) {
            clearTimeout(timer)
            off()
            resolve(item.id)
          }
        },
        (err) => {
          if (err.code !== 'resync') {
            clearTimeout(timer)
            reject(err)
          }
        },
      )
    })
    // Give the socket a moment to open before posting.
    await new Promise((r) => setTimeout(r, 500))
    const posted = await g.thread.send({ addr: fx.room.handle, text })
    assert(posted.ok === true && typeof posted.seq === 'number', JSON.stringify(posted))
    assert(posted.author?.kind === 'guest' && posted.author.login === fx.guest.username, JSON.stringify(posted.author))
    const id = await got
    assert(id === posted.id, `frame id ${id} vs post ${posted.id}`)
    const page = await g.thread.page({ addr: fx.room.handle, sinceSeq: posted.seq - 1 })
    assert(page.items.some((i) => i.id === posted.id), `the post is on the page: ${JSON.stringify({ posted, page }).slice(0, 900)}`)
  })
  if (fx.card) {
    const card = fx.card
    await check('thread: POST /cli/thread/answer', async () => {
      const r = await g.thread.answerCard({ addr: fx.room.handle, cardId: card.id, answer: card.answer })
      assert(r.ok === true && r.status === 'answered' && r.answer === card.answer, JSON.stringify(r))
    })
  } else {
    notRun.push({ name: 'thread: POST /cli/thread/answer', why: 'no pending card in the fixture' })
  }
  await check('thread: GET /cli/thread/latest for the guest (R5)', async () => {
    const r = await g.thread.latest({ addrs: [fx.room.handle] })
    const item = r.items[0]
    assert(item && item.addr === fx.room.handle && !item.error, JSON.stringify(r))
  })

  // ── files with the guest pass (§14.2; AR40 upload-chunk) ─────────
  const dir = `contract-${Date.now()}`
  await check('fs: write, read, list, rename with the guest pass', async () => {
    await g.files.create({ workspace: fx.room.handle, relPath: dir, isDirectory: true })
    const w = await g.files.write({ workspace: fx.room.handle, relPath: `${dir}/a.txt`, content: 'hello' })
    assert(w.success === true, JSON.stringify(w))
    const f = await g.files.read({ workspace: fx.room.handle, relPath: `${dir}/a.txt` })
    assert(f.content === 'hello', JSON.stringify(f))
    const list = await g.files.list({ workspace: fx.room.handle, relPath: dir })
    assert(list.some((e) => e.relPath === `${dir}/a.txt` && !e.isDirectory), JSON.stringify(list))
    const r = await g.files.rename({ workspace: fx.room.handle, relPath: `${dir}/a.txt`, newName: 'b.txt' })
    assert(r.relPath === `${dir}/b.txt`, JSON.stringify(r))
  })
  await check('fs: POST /cli/fs/upload-chunk in two chunks, then read-range', async () => {
    const one = Buffer.from('chunk-one;').toString('base64')
    const two = Buffer.from('chunk-two').toString('base64')
    const a = await g.files.uploadChunk({ workspace: fx.room.handle, dir, filename: 'up.bin', uploadId: 'u1', offset: 0, base64: one, totalBytes: 19 })
    assert(a.done === false && a.received === 10, JSON.stringify(a))
    const b = await g.files.uploadChunk({ workspace: fx.room.handle, dir, filename: 'up.bin', uploadId: 'u1', offset: 10, base64: two, isLast: true })
    assert(b.done === true && b.relPath === `${dir}/up.bin`, JSON.stringify(b))
    const r = await g.files.readRange({ workspace: fx.room.handle, relPath: `${dir}/up.bin`, offset: 6 })
    assert(Buffer.from(r.base64, 'base64').toString() === 'one;chunk-two' && r.eof === true && r.size === 19, JSON.stringify(r))
  })
  await check('fs: a room the guest does not hold is skin_room', async () => {
    const e = await g.files.list({ workspace: 'no-such-room-contract', relPath: '.' }).catch((x: unknown) => x)
    assert(e instanceof K2Error && e.code === 'skin_room' && e.status === 403, describeErr(e))
  })

  // ── k2 db (§14.2) ────────────────────────────────────────────────
  if (fx.db) {
    const table = fx.db.table
    const marker = `contract-${Date.now()}`
    await check('db: POST /cli/db/rows inserts as the guest', async () => {
      const r = await g.db.insert({ workspace: fx.room.handle, table, row: { note: marker } })
      assert(r.ok === true && r.row.note === marker, JSON.stringify(r))
    })
    await check('db: GET /cli/db/rows', async () => {
      const r = await g.db.rows({ workspace: fx.room.handle, table, limit: 500 })
      assert(Array.isArray(r.rows) && r.rows.some((row) => row.note === marker), JSON.stringify(r).slice(0, 400))
    })
    await check('db: POST /cli/db/query', async () => {
      const r = await g.db.query({ workspace: fx.room.handle, sql: `select note from ${table} where note = $1`, params: [marker] })
      assert(r.rowCount === 1 && r.rows[0]?.note === marker, JSON.stringify(r))
    })
  } else {
    for (const n of ['db: POST /cli/db/rows inserts as the guest', 'db: GET /cli/db/rows', 'db: POST /cli/db/query']) {
      notRun.push({ name: n, why: 'no workspace Postgres on this box (k2 db is Linux only)' })
    }
  }

  // ── the identity env K2 sets (AR40) ──────────────────────────────
  if (fx.checkEnv) {
    await check('env: K2_APP_ID, K2_HOST, K2_PORT, K2_GATEWAY_URL are set', async () => {
      for (const k of ['K2_APP_ID', 'K2_HOST', 'K2_PORT', 'K2_GATEWAY_URL']) assert(env[k], `${k} is not set`)
      assert(env.K2_APP_ID === wantApp, `K2_APP_ID ${env.K2_APP_ID} vs ${wantApp}`)
    })
  }
  return finish(results, notRun)
}

function finish(results: CheckResult[], notRun: Array<{ name: string; why: string }>, stopped?: string): SuiteResult {
  if (stopped) notRun.push({ name: '(the rest)', why: stopped })
  const failed = results.filter((r) => !r.ok)
  return { ok: failed.length === 0 && !stopped, passed: results.length - failed.length, failed, notRun, results }
}
