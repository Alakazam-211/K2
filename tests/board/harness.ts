// Shared harness for the Board widget's browser specs: serves crates/.../builtin_widgets/board-1 as a
// static site on http://localhost:58123/ (Playwright routes it: no server, and localhost is a secure context like K2's frame), with K2's k2-png@1 library and the
// shared stand-in `k2` (tests/board/fake-k2.js) loaded before the widget's own scripts, as K2's frame does.
import { readFileSync } from 'node:fs'
import { extname, resolve } from 'node:path'
import type { Page } from '@playwright/test'

const ROOT = resolve(__dirname, '../..')
export const DIR = resolve(ROOT, 'crates/k2-core/src/zen/builtin_widgets/board-1')
export const FAKE = readFileSync(resolve(__dirname, 'fake-k2.js'), 'utf8')
export const K2PNG = readFileSync(resolve(ROOT, 'src/renderer/zen-lib/k2-png@1.0.0/k2-png.js'), 'utf8')
export const PNG_1x1 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
const TYPES: Record<string, string> = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json' }

/* eslint-disable @typescript-eslint/no-explicit-any */
export type Opts = Record<string, any>

// Open the board page with a fake K2 built from opts (see fake-k2.js). Resolves once the first board is open.
export async function openBoard(page: Page, opts: Opts = {}, wait: { board?: string; objects?: number } = {}): Promise<void> {
  await page.setViewportSize({ width: 1400, height: 900 })
  await page.route('http://localhost:58123/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace(/^\/+/, '') || 'index.html'
    if (!/^[a-z0-9.-]+$/.test(path)) return route.fulfill({ status: 404, body: 'not found' })
    let body: string
    try { body = readFileSync(resolve(DIR, path), 'utf8') } catch { return route.fulfill({ status: 404, body: 'not found' }) }
    return route.fulfill({ status: 200, contentType: TYPES[extname(path)] || 'application/octet-stream', body })
  })
  await page.addInitScript({ content: 'window.__errors = []; addEventListener("error", (e) => window.__errors.push(String(e.message)))' })
  await page.addInitScript({ content: K2PNG })
  await page.addInitScript({ content: FAKE })
  await page.addInitScript((o: Opts) => {
    const w = window as any
    // Stored files show as real pictures: an object URL of their bytes.
    o.urlOf = (_ref: string, _px: number, bytes: Uint8Array, mime: string) => URL.createObjectURL(new Blob([bytes], { type: mime }))
    w.__fake = w.makeFakeK2(o)
    w.k2 = w.__fake.k2
  }, opts)
  await page.goto('http://localhost:58123/index.html')
  await page.waitForFunction(({ board, objects }) => {
    const w = window as any
    return w.__fake.log.ready >= 1 && w.BoardApp && w.BoardApp.store.boardId && (!board || w.BoardApp.store.boardId === board) && (!objects || w.BoardApp.model.all().length >= objects)
  }, wait)
}

// Board's own checks are written as ok(name, cond, extra): collect them, then fail loudly with every miss.
export function collector(): { ok: (name: string, cond: unknown, extra?: unknown) => void; failures: () => string[]; count: () => number } {
  const failed: string[] = []; let n = 0
  return {
    ok(name, cond, extra) { n++; if (!cond) failed.push(name + (extra !== undefined ? '  ' + JSON.stringify(extra) : '')) },
    failures: () => failed,
    count: () => n,
  }
}

// The three-sticky board the keyboard and feature checks start from (boards b1, b2, b3; b1 open).
export const sticky = (id: string, x: number, y: number, text: string): Opts => ({ id, type: 'sticky', x, y, w: 200, h: 200, text, color: 'yellow', z: 1 })
export function threeBoards(objects: Opts[]): Opts {
  const seed: Array<[string, unknown]> = [
    ['seeded', { at: '2026-01-01T00:00:00Z' }],
    ['boards/b1', { id: 'b1', name: 'Test', project: null, format: 1, order: 0 }],
    ['boards/b2', { id: 'b2', name: 'Second', project: null, format: 1, order: 1 }],
    ['boards/b3', { id: 'b3', name: 'Third', project: null, format: 1, order: 2 }],
    ['board/b1/meta', { format: 1, title: 'Test' }],
  ]
  for (const o of objects) seed.push(['board/b1/obj/' + o.id, o])
  return { seed, saves: [['prefs', { open: 'b1' }]] }
}
