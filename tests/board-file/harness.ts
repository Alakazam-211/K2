// k2:board-file@2 in real browsers. The widget's folder is served as a static site by Playwright's router, with the
// shared stand-in `k2` (tests/board/fake-k2.js) for what a frame gets from K2, and `k2.boardFile` wired to the REAL
// session (src/renderer/lib/board-file) over a fake daemon that follows board-tab-b3-interface.md. So everything from a
// pointer in WebKit to the body of a save is the shipped code; only the daemon is a stand-in.
import { buildSync } from 'esbuild'
import { readFileSync } from 'node:fs'
import { extname, resolve } from 'node:path'
import type { Page } from '@playwright/test'

const ROOT = resolve(__dirname, '../..')
export const DIR = resolve(ROOT, 'crates/k2-core/src/zen/builtin_widgets/board-file-2')
const FAKE = readFileSync(resolve(ROOT, 'tests/board/fake-k2.js'), 'utf8')
const K2PNG = readFileSync(resolve(ROOT, 'src/renderer/zen-lib/k2-png@1.0.1/k2-png.js'), 'utf8')
const TYPES: Record<string, string> = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json' }

const BF = buildSync({
  entryPoints: [resolve(__dirname, 'page-entry.ts')],
  bundle: true,
  write: false,
  format: 'iife',
  platform: 'browser',
  alias: { '@': resolve(ROOT, 'src/renderer') },
}).outputFiles[0].text

/* eslint-disable @typescript-eslint/no-explicit-any */
export type Opts = Record<string, any>

export const sticky = (id: string, x: number, y: number, text = 't'): Opts => ({ id, type: 'sticky', x, y, w: 200, h: 200, text, color: 'yellow' })

export async function openBoardFile(page: Page, file: Opts = {}, opts: { viewport?: { width: number; height: number } } = {}): Promise<void> {
  await page.setViewportSize(opts.viewport ?? { width: 1400, height: 900 })
  await page.route('http://localhost:58124/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace(/^\/+/, '') || 'index.html'
    if (!/^[a-z0-9.-]+$/.test(path)) return route.fulfill({ status: 404, body: 'not found' })
    let body: string
    try {
      body = readFileSync(resolve(DIR, path), 'utf8')
    } catch {
      return route.fulfill({ status: 404, body: 'not found' })
    }
    return route.fulfill({ status: 200, contentType: TYPES[extname(path)] || 'application/octet-stream', body })
  })
  await page.addInitScript({ content: 'window.__errors = []; addEventListener("error", (e) => window.__errors.push(String(e.message))); addEventListener("unhandledrejection", (e) => window.__errors.push(String(e.reason)))' })
  await page.addInitScript({ content: K2PNG })
  await page.addInitScript({ content: FAKE })
  await page.addInitScript({ content: BF })
  await page.addInitScript((file: Opts) => {
    const w = window as any
    const d = w.BF.createFakeBoardDaemon(file)
    const s = w.BF.createBoardFileSession({ api: d, path: '/w/Plan.k5board', setTimer: (f: () => void, ms: number) => setTimeout(f, ms), clearTimer: (t: number) => clearTimeout(t), now: () => Date.now(), visible: () => true })
    const join = w.BF.createBoardFilePartJoiner()
    const fake = w.makeFakeK2({ caps: ['blobs.put', 'blobs.get', 'downloads.save', 'uploads.pick', 'links.open', 'boardFile.open'] })
    const log = { keys: [] as unknown[], parts: 0, largestPart: 0, holds: [] as boolean[], submits: [] as unknown[] }
    fake.k2.widget = { id: 'k2:board-file@2', name: 'Board', garden: 'board-file' }
    fake.k2.connected = s.start()
    fake.k2.boardFile = {
      open: async () => JSON.parse(JSON.stringify(s.open())),
      submit: async (p: any) => {
        const size = JSON.stringify({ verb: 'boardFile.submit', args: [p] }).length
        log.parts += 1
        log.largestPart = Math.max(log.largestPart, size)
        if (size > 64 * 1024) throw Object.assign(new Error('A message is over 64 KB.'), { code: 'too_large' })
        const text = join(p)
        if (text !== null) {
          log.submits.push({ kind: p.kind, byAll: p.byAll, ops: JSON.parse(text) })
          s.submit({ gesture: p.gesture, kind: p.kind, byAll: p.byAll, ops: JSON.parse(text) })
        }
        return { ok: true }
      },
      hold: async (on: boolean) => { log.holds.push(on); s.hold(on); return { ok: true } },
      flush: async () => { await s.flush(); return { ok: true } },
      key: async (k: unknown) => { log.keys.push(k); return { ok: true } },
      events: (cb: (e: unknown) => void) => s.onEvent((e: unknown) => cb(JSON.parse(JSON.stringify(e)))),
    }
    w.k2 = fake.k2
    w.__fake = fake
    w.__d = d
    w.__s = s
    w.__log = log
  }, file)
  await page.goto('http://localhost:58124/index.html')
  await page.waitForFunction(() => (window as any).__fake.log.ready >= 1 && (window as any).BoardApp && (window as any).BoardApp.store.boardId === 'file')
}

export function collector(): { ok: (name: string, cond: unknown, extra?: unknown) => void; failures: () => string[]; count: () => number } {
  const failed: string[] = []
  let n = 0
  return {
    ok(name, cond, extra) {
      n++
      if (!cond) failed.push(name + (extra !== undefined ? '  ' + JSON.stringify(extra) : ''))
    },
    failures: () => failed,
    count: () => n,
  }
}
