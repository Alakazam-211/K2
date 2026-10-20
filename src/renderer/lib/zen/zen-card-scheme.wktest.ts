// 0.46.3 (Garden gaps): K2's built-in source cards follow the Garden's
// light/dark theme, never the computer's. Rendered in headless WebKit (the
// desktop webview's engine): the frame is built with the real prelude
// (`zenFramePrelude`), the card kit and card files as shipped, inside a
// parent whose color-scheme and background are the Garden's (as ZenRoot
// sets them). Playwright emulates the computer's appearance. A pixel in the
// card's empty corner must be the Garden's background: an opaque frame
// canvas (white on a dark Garden in Light mode) is the bug.
//
//   bun run test:webkit   (npx playwright install webkit first)

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { webkit, type Browser } from 'playwright'
import { PNG } from 'pngjs'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { zenFramePrelude } from './zen-custom-prelude'

const root = resolve(__dirname, '../../../..')
const widgets = resolve(root, 'crates/k2-core/src/zen/builtin_widgets')
/** ZEN_SHOTS=<dir> keeps each render as a PNG (for the report). */
const shots = process.env.ZEN_SHOTS

type Scheme = 'light' | 'dark'
interface Case {
  card: string
  kit: string
  garden: Scheme
  os: Scheme
}

const GARDEN = {
  dark: {
    bg: [16, 20, 26],
    vars: { '--zen-text': '#e8eaed', '--zen-text-muted': '#9aa0a6', '--zen-surface-raised': 'rgba(255,255,255,0.06)', '--zen-border': 'rgba(255,255,255,0.14)' },
  },
  light: {
    bg: [246, 244, 238],
    vars: { '--zen-text': '#1f2328', '--zen-text-muted': '#6b717a', '--zen-surface-raised': 'rgba(0,0,0,0.04)', '--zen-border': 'rgba(0,0,0,0.12)' },
  },
} as const

const now = Math.floor(Date.now() / 1000)
const VIEW = {
  name: 'fleet',
  label: 'Fleet',
  health: 'ok',
  phase: 'live',
  display: 'ok',
  fields: { target_version: '0.46.2' },
  items: [
    { key: 'ohana', health: 'ok', fields: { version: '0.46.2', phase: 'ready' } },
    { key: 'luzz', health: 'warn', fields: { version: '0.45.7', phase: 'ready' }, soft: { count: 1, of: 3, level: 'crit' } },
  ],
  counts: { ok: 1, warn: 1, crit: 0, unknown: 0 },
  soft: { count: 0, of: 3 },
  serverNow: now,
  updatedAt: now - 20,
}

function frameDoc(c: Case, prelude: (p: string) => string): string {
  const kit = resolve(widgets, c.kit)
  const css = readFileSync(resolve(kit, 'kit.css'), 'utf8')
  const js = [readFileSync(resolve(kit, 'kit.js'), 'utf8'), readFileSync(resolve(widgets, c.card, 'card.js'), 'utf8')]
  const head = prelude(zenFramePrelude({ nonce: 'n0', theme: { vars: { ...GARDEN[c.garden].vars }, scheme: c.garden }, libs: [] }))
  return `<!doctype html><html><head><meta charset="utf-8">${head}<style>${css}</style></head><body><div id="card"></div>${js
    .map((t) => `<script>${t.replace(/<\/(script)/gi, '<\\/$1')}</script>`)
    .join('')}</body></html>`
}

function parentDoc(c: Case, frame: string): string {
  const [r, g, b] = GARDEN[c.garden].bg
  const hello = { k2: 'hello', v: 1, caps: ['sources:read'], features: ['zen-v1'], widget: { id: 'p1', name: c.card, garden: 'g-test' }, config: { source: 'fleet', title: 'Fleet' } }
  return `<!doctype html><html style="color-scheme:${c.garden}"><body style="margin:0;background:rgb(${r},${g},${b})">
<iframe id="f" style="border:0;width:420px;height:360px;display:block;color-scheme:${c.garden}"></iframe>
<script>
const f = document.getElementById('f')
f.addEventListener('load', () => {
  const ch = new MessageChannel()
  ch.port1.onmessage = (e) => {
    const m = e.data
    if (m && typeof m.sub === 'number' && m.verb === 'sources.subscribe') ch.port1.postMessage({ sub: m.sub, value: ${JSON.stringify(VIEW)} })
    if (m && m.ready) window.__ready = true
  }
  f.contentWindow.postMessage(${JSON.stringify(hello)}, '*', [ch.port2])
})
f.srcdoc = ${JSON.stringify(frame).replace(/<\//g, '<\\/')}
</script></body></html>`
}

interface Render {
  corner: number[]
  scheme: string
  title: string
}

let browser: Browser
beforeAll(async () => {
  browser = await webkit.launch({ headless: true })
})
afterAll(async () => {
  await browser?.close()
})

async function render(c: Case, prelude: (p: string) => string = (p) => p, tag = ''): Promise<Render> {
  const page = await browser.newPage({ colorScheme: c.os, viewport: { width: 440, height: 380 } })
  try {
    await page.setContent(parentDoc(c, frameDoc(c, prelude)))
    await page.waitForFunction(() => (window as unknown as { __ready?: boolean }).__ready === true, null, { timeout: 15_000 })
    await page.waitForTimeout(150)
    const shot = await page.screenshot({ clip: { x: 0, y: 0, width: 420, height: 360 } })
    if (shots) {
      mkdirSync(shots, { recursive: true })
      writeFileSync(resolve(shots, `${c.card}-${c.garden}-garden-${c.os}-os${tag}.png`), shot)
    }
    const png = PNG.sync.read(shot)
    const i = (png.width * (png.height - 8) + (png.width - 8)) * 4
    const frame = page.frameLocator('#f')
    return {
      corner: [png.data[i], png.data[i + 1], png.data[i + 2]],
      scheme: await frame.locator('html').evaluate((el) => getComputedStyle(el).colorScheme),
      title: await frame.locator('.k2c-title').first().evaluate((el) => getComputedStyle(el).color),
    }
  } finally {
    await page.close()
  }
}

const near = (a: number[], b: readonly number[]): boolean => a.every((v, i) => Math.abs(v - b[i]) <= 6)

const CARDS: Array<[string, string]> = [
  ['fleet-grid-2', 'sources-kit-2'],
  ['version-spread-2', 'sources-kit-2'],
  ['money-alerts-2', 'sources-kit-2'],
  ['fleet-grid-1', 'sources-kit'],
  ['version-spread-1', 'sources-kit'],
]

describe('built-in cards follow the Garden’s light/dark, not the computer’s (WebKit)', () => {
  for (const [card, kit] of CARDS) {
    it(`${card}: a dark Garden on a Light-mode computer is dark`, async () => {
      const r = await render({ card, kit, garden: 'dark', os: 'light' })
      expect(r.scheme).toBe('dark')
      expect(near(r.corner, GARDEN.dark.bg), `corner rgb(${r.corner.join(',')}) is not the dark Garden`).toBe(true)
      expect(r.title).toBe('rgb(232, 234, 237)')
    })
    it(`${card}: a light Garden on a Dark-mode computer is light`, async () => {
      const r = await render({ card, kit, garden: 'light', os: 'dark' })
      expect(r.scheme).toBe('light')
      expect(near(r.corner, GARDEN.light.bg), `corner rgb(${r.corner.join(',')}) is not the light Garden`).toBe(true)
      expect(r.title).toBe('rgb(31, 35, 40)')
    })
  }

  it('the check catches the bug: without !important a dark Garden on a Light-mode computer is not the Garden', async () => {
    const r = await render({ card: 'fleet-grid-2', kit: 'sources-kit-2', garden: 'dark', os: 'light' }, (p) => p.replace('!important', ''), '-before')
    expect(r.scheme).toBe('light dark')
    expect(near(r.corner, GARDEN.dark.bg), `before the fix the corner was rgb(${r.corner.join(',')})`).toBe(false)
  })
})
