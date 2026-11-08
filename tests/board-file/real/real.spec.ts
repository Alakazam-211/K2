// k2:board-file@1 on a REAL .k5board, in WebKit and Chromium: the real daemon (sandbox HOME, built from this commit),
// the format's codec, the real session and the real widget. Only the frame's `k2` is the stand-in in fake-k2.js, with
// `k2.boardFile` wired to the session, as the tab's layer wires it. The page's API calls go to the daemon through
// Playwright's router, which adds the owner token as the desktop app's own requests do.
//
//   K2_BOARD_DAEMON_BIN=<k2-daemon> npx playwright test --config tests/board-file/real/playwright.config.ts
/* eslint-disable @typescript-eslint/no-explicit-any */
import { expect, test, type Page } from '@playwright/test'
import { buildSync } from 'esbuild'
import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, statSync } from 'node:fs'
import { extname, join, resolve } from 'node:path'
import { createSandbox, type Sandbox } from './daemon'

const ROOT = resolve(__dirname, '../../..')
const DIR = resolve(ROOT, 'crates/k2-core/src/zen/builtin_widgets/board-file-1')
const FAKE = readFileSync(resolve(ROOT, 'tests/board/fake-k2.js'), 'utf8')
const K2PNG = readFileSync(resolve(ROOT, 'src/renderer/zen-lib/k2-png@1.0.1/k2-png.js'), 'utf8')
const TYPES: Record<string, string> = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json' }
const ORIGIN = 'http://localhost:58125'
const BF = buildSync({ entryPoints: [resolve(__dirname, 'page-entry.ts')], bundle: true, write: false, format: 'iife', platform: 'browser', alias: { '@': resolve(ROOT, 'src/renderer') } }).outputFiles[0].text

declare const BoardApp: any
declare const __s: any
declare const __errors: string[]

test.describe.configure({ mode: 'serial' })
let sb: Sandbox
let ws: string
let file: string
const sticky = (id: string, x: number, y: number, text: string): any => ({ id, type: 'sticky', x, y, w: 200, h: 200, text, color: 'yellow' })

test.beforeAll(async () => {
  sb = createSandbox()
  await sb.start()
  const on = await sb.post('settings/update', { boardFilePreview: true })
  expect(on.status).toBe(200)
  ws = join(sb.work, 'alpha')
  mkdirSync(ws, { recursive: true })
  // A repo, as tests/cli/board_file_headless.sh registers its workspaces.
  execFileSync('git', ['-c', 'init.defaultBranch=main', '-C', ws, 'init', '-q'], { env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' } })
  const reg = await sb.post('projects/add-from-path', { path: ws, seedWiki: false, seedAgentsMd: false })
  expect(reg.status, JSON.stringify(reg.body)).toBe(200)
  expect(JSON.stringify(reg.body), 'the workspace is registered').toContain(ws)
})
test.afterAll(async () => {
  await sb.stop()
  sb.dispose()
})

async function openTab(page: Page, path: string): Promise<void> {
  await page.setViewportSize({ width: 1400, height: 900 })
  await page.route(`${ORIGIN}/**`, async (route) => {
    const url = new URL(route.request().url())
    if (url.pathname.startsWith('/cli/')) {
      url.searchParams.set('token', sb.token)
      const r = await fetch(`${sb.base}${url.pathname}?${url.searchParams}`, { method: route.request().method(), headers: { 'Content-Type': 'application/json' }, body: route.request().postData() ?? undefined })
      return route.fulfill({ status: r.status, contentType: 'application/json', body: await r.text() })
    }
    const p = url.pathname.replace(/^\/+/, '') || 'index.html'
    if (!/^[a-z0-9.-]+$/.test(p)) return route.fulfill({ status: 404, body: 'not found' })
    return route.fulfill({ status: 200, contentType: TYPES[extname(p)] || 'application/octet-stream', body: readFileSync(resolve(DIR, p), 'utf8') })
  })
  await page.addInitScript({ content: 'window.__errors = []; addEventListener("error", (e) => window.__errors.push(String(e.message))); addEventListener("unhandledrejection", (e) => window.__errors.push(String(e.reason)))' })
  await page.addInitScript({ content: K2PNG })
  await page.addInitScript({ content: FAKE })
  await page.addInitScript({ content: BF })
  await page.addInitScript((path: string) => {
    const w = window as any
    const api = {
      async get(route: string, params: Record<string, string | number | undefined>) {
        const q = new URLSearchParams()
        for (const [k, v] of Object.entries(params)) if (v !== undefined) q.set(k, String(v))
        return w.BF.answer(await fetch(`/cli/${route}?${q}`))
      },
      async post(route: string, body: unknown) {
        return w.BF.answer(await fetch(`/cli/${route}`, { method: 'POST', body: JSON.stringify(body) }))
      },
    }
    const s = w.BF.createBoardFileSession({ api, path, setTimer: (f: () => void, ms: number) => setTimeout(f, ms), clearTimer: (t: number) => clearTimeout(t), now: () => Date.now(), visible: () => true })
    const join = w.BF.createBoardFilePartJoiner()
    const fake = w.makeFakeK2({ caps: ['blobs.put', 'blobs.get', 'downloads.save', 'uploads.pick', 'links.open'] })
    fake.k2.widget = { id: 'k2:board-file@1', name: 'Board', garden: 'board-file' }
    fake.k2.connected = s.start()
    fake.k2.boardFile = {
      open: async () => JSON.parse(JSON.stringify(s.open())),
      submit: async (p: any) => { const t = join(p); if (t !== null) s.submit({ gesture: p.gesture, kind: p.kind, byAll: p.byAll, ops: JSON.parse(t) }); return { ok: true } },
      hold: async (on: boolean) => { s.hold(on); return { ok: true } },
      flush: async () => { await s.flush(); return { ok: true } },
      key: async () => ({ ok: true }),
      events: (cb: (e: unknown) => void) => s.onEvent((e: unknown) => cb(JSON.parse(JSON.stringify(e)))),
    }
    w.k2 = fake.k2
    w.__s = s
  }, path)
  await page.goto(`${ORIGIN}/index.html`)
  await page.waitForFunction(() => (window as any).BoardApp && (window as any).BoardApp.store.boardId === 'file')
}

const fileObjects = async (): Promise<any[]> => {
  const r = await sb.get('board/file/read', { path: file })
  expect(r.status, JSON.stringify(r.body)).toBe(200)
  return r.body.objects
}

test('the + menu’s door makes Untitled.k5board, a whole file, and the bundle route serves the tab’s widget with no Garden', async () => {
  const a = await sb.post('board/file/create', { dir: ws })
  expect(a.status, JSON.stringify(a.body)).toBe(200)
  expect(a.body.name).toBe('Untitled.k5board')
  file = a.body.path
  expect(statSync(file).size).toBeGreaterThan(0)
  const b = await sb.post('board/file/create', { dir: ws })
  expect(b.body.name).toBe('Untitled-2.k5board')
  const bundle = await sb.get('zen/widget/bundle', { widget: 'k2:board-file@1' })
  expect(bundle.status).toBe(200)
  expect(String(bundle.body.html)).toContain('<title>Board</title>')
  expect(String(bundle.body.html)).toContain('k2.boardFile.submit(')
})

test('an agent’s writes draw, and opening writes nothing', async ({ page }) => {
  const r0 = await sb.get('board/file/read', { path: file })
  const seeded = await sb.post('board/file/save', {
    path: file,
    baseHash: r0.body.hash,
    groups: [{ id: 'seed-1', ops: [{ op: 'add', obj: sticky('s-00000000a1', 200, 150, 'from the file'), at: 'front' }, { op: 'add', obj: sticky('s-00000000a2', 500, 150, 'second'), at: 'front' }] }],
  })
  expect(seeded.status, JSON.stringify(seeded.body)).toBe(200)
  const before = (await sb.get('board/file/info', { path: file })).body.hash
  await openTab(page, file)
  await page.waitForFunction(() => BoardApp.model.all().length === 2)
  expect(await page.evaluate(() => BoardApp.model.get('s-00000000a1').text)).toBe('from the file')
  await expect(page.locator('.boardname .name')).toContainText('Untitled')
  await page.waitForTimeout(1500)
  expect((await sb.get('board/file/info', { path: file })).body.hash).toBe(before)
  expect(await page.evaluate(() => __errors)).toEqual([])
})

test('a person’s typing and drag land in the file, and only the fields they named', async ({ page }) => {
  await openTab(page, file)
  await page.waitForFunction(() => BoardApp.model.all().length === 2)
  await page.mouse.dblclick(900, 600)
  await page.keyboard.type('typed in a tab')
  await page.keyboard.press('Escape')
  await page.keyboard.press('Meta+s')
  await expect.poll(async () => (await fileObjects()).some((o) => o.text === 'typed in a tab'), { timeout: 10_000 }).toBe(true)
  const at = await page.evaluate(() => { const o = BoardApp.model.get('s-00000000a1'); return [o.x + 50 + BoardApp.cam.x, o.y + 50 + BoardApp.cam.y] })
  await page.mouse.move(at[0], at[1])
  await page.mouse.down()
  await page.mouse.move(at[0] + 60, at[1] + 40, { steps: 6 })
  await page.mouse.up()
  await page.keyboard.press('Meta+s')
  await expect.poll(async () => (await fileObjects()).find((o) => o.id === 's-00000000a1').x, { timeout: 10_000 }).not.toBe(200)
  const moved = (await fileObjects()).find((o) => o.id === 's-00000000a1')
  expect(moved.text).toBe('from the file')
  expect(moved.color).toBe('yellow')
  expect(await page.evaluate(() => __s.view().status.state)).toBe('saved')
})

test('a conflict holds the edit; it survives a daemon restart; Use mine lands it', async ({ page }) => {
  await openTab(page, file)
  await page.waitForFunction(() => BoardApp.model.all().length === 3)
  // Someone else changes the sticky, and the tab hasn't heard.
  const r = await sb.get('board/file/read', { path: file })
  const theirs = await sb.post('board/file/save', { path: file, baseHash: r.body.hash, groups: [{ id: 'other-1', ops: [{ op: 'set', id: 's-00000000a2', rev: r.body.revs['s-00000000a2'], f: { text: 'theirs' } }] }] })
  expect(theirs.status, JSON.stringify(theirs.body)).toBe(200)
  await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-00000000a2', f: { text: 'mine' } }]))
  await page.evaluate(() => __s.flush())
  await page.waitForFunction(() => __s.view().held.length === 1)
  const held = await page.evaluate(() => __s.view().held[0])
  expect(held.heldId, 'the daemon keeps it').toMatch(/^[0-9a-f]{32}$/)
  expect(held.objects[0].fields).toEqual([{ field: 'text', mine: 'mine', theirs: 'theirs' }])
  expect((await fileObjects()).find((o) => o.id === 's-00000000a2').text).toBe('theirs')
  await page.close()

  // K2 restarts: this tab's memory is gone, the daemon's store isn't.
  await sb.stop()
  await sb.start()
  const page2 = await page.context().newPage()
  await openTab(page2, file)
  await page2.waitForFunction(() => __s.view().held.length === 1)
  const back = await page2.evaluate(() => __s.view().held[0])
  expect(back.heldId).toBe(held.heldId)
  expect(back.objects[0].fields).toEqual([{ field: 'text', mine: 'mine', theirs: 'theirs' }])
  await page2.evaluate((id) => __s.useMine(id), back.id)
  await page2.evaluate(() => __s.flush())
  await expect.poll(async () => (await fileObjects()).find((o) => o.id === 's-00000000a2').text, { timeout: 10_000 }).toBe('mine')
  await page2.waitForFunction(() => __s.view().held.length === 0)
  const after = await sb.get('board/file/read', { path: file })
  expect(after.body.held ?? []).toEqual([])
})

test('Save a copy elsewhere, and the original is left as it was', async ({ page }) => {
  await openTab(page, file)
  await page.waitForFunction(() => BoardApp.model.all().length === 3)
  const before = (await sb.get('board/file/info', { path: file })).body.hash
  // Outside every workspace (the owner may, so a copy can leave a full disk or a gone share): the file is there.
  const away = join(sb.work, 'Away.k5board')
  const r0 = await page.evaluate((to) => __s.saveCopy(to), away)
  expect(r0.ok, JSON.stringify(r0)).toBe(true)
  expect(statSync(away).size).toBeGreaterThan(0)
  // Inside one, so the daemon reads it back: the whole board.
  const to = join(ws, 'Copy.k5board')
  const r = await page.evaluate((to) => __s.saveCopy(to), to)
  expect(r.ok, JSON.stringify(r)).toBe(true)
  const copy = await sb.get('board/file/read', { path: to })
  expect(copy.status, JSON.stringify(copy.body)).toBe(200)
  expect(copy.body.objects).toHaveLength(3)
  expect((await sb.get('board/file/info', { path: file })).body.hash).toBe(before)
})
