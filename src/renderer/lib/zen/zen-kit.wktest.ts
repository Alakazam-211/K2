// 0.46.3 (Garden gaps): K2's widget kit rendered in headless WebKit (the
// desktop webview's engine). A widget frame is built with the real prelude
// (`zenFramePrelude`, the Garden's --zen-* variables and scheme) and the
// shipped library files, inside a parent that answers the frame runtime's
// port like K2 does (hello, agents.subscribe, thread.subscribe). Each Thread
// look renders in a dark and a light Garden, the opposite of the computer's
// appearance, and must paint the Garden's colours: the bubble fill read
// back from pixels, the chamfer cut (chamfer) or the round corners (glass),
// the frame's color-scheme. The (i) help renders open in both schemes.
//
//   bun run test:webkit   (npx playwright install webkit first)
//   ZEN_SHOTS=<dir> keeps one PNG per render.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { webkit, type Browser, type Page } from 'playwright'
import { PNG } from 'pngjs'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { zenFramePrelude } from './zen-custom-prelude'

const LIB = resolve(__dirname, '../../zen-lib')
const THREAD = readFileSync(resolve(LIB, 'k2-thread@1.0.0/k2-thread.js'), 'utf8')
const HELP = readFileSync(resolve(LIB, 'k2-help@1.0.0/k2-help.js'), 'utf8')
const shots = process.env.ZEN_SHOTS

type Scheme = 'light' | 'dark'
type Look = 'chamfer' | 'glass'

// K2's basic theme (crates/k2-core/src/zen/themes/basic.toml) as --zen-*.
const BASIC: Record<Scheme, Record<string, string>> = {
  light: {
    '--zen-canvas': '#f7f8fa', '--zen-surface': '#ffffff', '--zen-surface-raised': '#ffffff', '--zen-border': '#e3e6ec',
    '--zen-text': '#1a1d23', '--zen-text-muted': '#5f6672', '--zen-accent': '#2563eb', '--zen-accent-text': '#ffffff',
    '--zen-bubble-me': '#2563eb', '--zen-bubble-me-text': '#ffffff', '--zen-bubble-agent': '#eef1f5', '--zen-bubble-agent-text': '#1a1d23',
    '--zen-working': '#2563eb', '--zen-needs-you': '#c27c0e', '--zen-ok': '#2f855a', '--zen-danger': '#c53030',
    '--zen-radius': '14px', '--zen-bubble-radius': '18px', '--zen-gap': '12px', '--zen-font-size': '14px', '--zen-line-height': '1.45',
  },
  dark: {
    '--zen-canvas': '#121316', '--zen-surface': '#1a1b1f', '--zen-surface-raised': '#222328', '--zen-border': '#2e3036',
    '--zen-text': '#e6e8ec', '--zen-text-muted': '#9aa0ab', '--zen-accent': '#60a5fa', '--zen-accent-text': '#0b1220',
    '--zen-bubble-me': '#1d4ed8', '--zen-bubble-me-text': '#eff6ff', '--zen-bubble-agent': '#222328', '--zen-bubble-agent-text': '#e6e8ec',
    '--zen-working': '#60a5fa', '--zen-needs-you': '#e0a43a', '--zen-ok': '#5cc08a', '--zen-danger': '#f07167',
    '--zen-radius': '14px', '--zen-bubble-radius': '18px', '--zen-gap': '12px', '--zen-font-size': '14px', '--zen-line-height': '1.45',
  },
}

const hex = (h: string): number[] => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16))

/** The agent bubble's fill each look paints. */
const AGENT_FILL: Record<Look, Record<Scheme, string>> = {
  chamfer: { light: '#eef1f5', dark: '#222328' },
  glass: { light: '#e9e9eb', dark: '#3a3a3c' },
}
const ME_FILL: Record<Look, Record<Scheme, string>> = {
  chamfer: { light: '#2563eb', dark: '#1d4ed8' },
  glass: { light: '#007aff', dark: '#0a84ff' },
}

const AGENT = 'ada::rosson.k2.dev'
const now = Math.floor(Date.now() / 1000)
const ITEMS = [
  { seq: 1, id: 'm1', doc: { id: 'm1', kind: 'text', from: 'ada', to: null, created_at: now - 600, body: 'Morning. The 0.46.3 rollout is green on 9 of 11 boxes.', via: null, choice: null } },
  { seq: 2, id: 'm2', doc: { id: 'm2', kind: 'text', from: 'rosson', to: null, created_at: now - 540, body: 'Which two are behind?', via: 'compose', choice: null } },
  {
    seq: 3, id: 'm3',
    doc: { id: 'm3', kind: 'choice', from: 'ada', to: null, created_at: now - 120, body: null, via: null,
      choice: { prompt: 'luzz and ohana. Roll them now?', options: [{ label: 'Roll now' }, { label: 'Tonight' }], allow_custom: false, status: 'pending', answer: null, answeredBy: null, expiresAt: null } },
  },
]

function frameDoc(look: Look, scheme: Scheme, width: number): string {
  const prelude = zenFramePrelude({ nonce: 'n0', theme: { vars: { ...BASIC[scheme] }, scheme }, libs: [THREAD, HELP] })
  const widget = `
k2.connected.then(function () {
  var head = document.getElementById('head')
  head.appendChild(K2Help.tip('Talk to Ada here. Enter sends; Shift+Enter is a new line.', { label: 'this panel', key: 'h' }))
  window.__panel = K2Thread.mount(document.getElementById('t'), { look: ${JSON.stringify(look)}, address: ${JSON.stringify(AGENT)}, label: 'Ada' })
  var ta = document.querySelector('.k2t-field textarea')
  ta.value = 'Roll luzz first'
  ta.dispatchEvent(new Event('input', { bubbles: true }))
  k2.ready()
})`
  return `<!doctype html><html><head><meta charset="utf-8">${prelude}<style>html,body{margin:0;height:100%}
#head{display:flex;align-items:center;gap:4px;height:34px;padding:0 12px;font:600 13px/1 -apple-system,system-ui,sans-serif;color:var(--zen-text)}
#t{height:calc(100% - 34px);width:${width}px;padding:0 8px 8px}</style></head><body><div id="head">Thread</div><div id="t"></div>
<script>${widget}</script></body></html>`
}

function parentDoc(look: Look, scheme: Scheme, width: number): string {
  const bg = BASIC[scheme]['--zen-canvas']
  const hello = { k2: 'hello', v: 1, caps: ['agents:read', 'thread:read', 'thread:post'], features: ['zen-v1'], widget: { id: 'p1', name: 'thread', garden: 'g-test' }, config: {} }
  const view = { address: AGENT, phase: 'open', items: ITEMS, hasMore: false, turn: { state: 'working', since: Date.now() - 74_000 } }
  const rows = [{ address: AGENT, label: 'Ada', counts: { subagents: 1, tools: 4, commands: 2 } }]
  // `</` inside the script's string literal would end the parent's script.
  const frame = JSON.stringify(frameDoc(look, scheme, width)).replace(/<\//g, '<\\/')
  return `<!doctype html><html style="color-scheme:${scheme}"><body style="margin:0;background:${bg}">
<iframe id="f" style="border:0;width:${width + 16}px;height:560px;display:block;color-scheme:${scheme}"></iframe>
<script>
const f = document.getElementById('f')
f.addEventListener('load', () => {
  const ch = new MessageChannel()
  ch.port1.onmessage = (e) => {
    const m = e.data
    if (!m) return
    if (typeof m.sub === 'number' && m.verb === 'agents.subscribe') ch.port1.postMessage({ sub: m.sub, value: ${JSON.stringify(rows)} })
    if (typeof m.sub === 'number' && m.verb === 'thread.subscribe') setTimeout(() => ch.port1.postMessage({ sub: m.sub, value: ${JSON.stringify(view)} }), 20)
    if (m.ready) setTimeout(() => { window.__ready = true }, 400)
  }
  f.contentWindow.postMessage(${JSON.stringify(hello)}, '*', [ch.port2])
})
f.srcdoc = ${frame}
</script></body></html>`
}

let browser: Browser
beforeAll(async () => {
  browser = await webkit.launch({ headless: true })
})
afterAll(async () => {
  await browser?.close()
})

async function open(look: Look, scheme: Scheme, width = 420): Promise<{ page: Page; png: PNG; shot: Buffer }> {
  // The computer's appearance is the opposite of the Garden's.
  const page = await browser.newPage({ colorScheme: scheme === 'dark' ? 'light' : 'dark', viewport: { width: width + 40, height: 600 } })
  await page.setContent(parentDoc(look, scheme, width))
  await page.waitForFunction(() => (window as unknown as { __ready?: boolean }).__ready === true, null, { timeout: 15_000 })
  const shot = await page.screenshot({ clip: { x: 0, y: 0, width: width + 16, height: 560 } })
  return { page, png: PNG.sync.read(shot), shot }
}

function save(name: string, shot: Buffer): void {
  if (!shots) return
  mkdirSync(shots, { recursive: true })
  writeFileSync(resolve(shots, name), shot)
}

function pixel(png: PNG, x: number, y: number): number[] {
  const i = (png.width * Math.round(y) + Math.round(x)) * 4
  return [png.data[i], png.data[i + 1], png.data[i + 2]]
}
const near = (a: number[], b: number[], tol = 4): boolean => a.every((v, i) => Math.abs(v - b[i]) <= tol)

describe('k2-thread@1 in WebKit: each look follows the Garden, not the computer', () => {
  for (const look of ['chamfer', 'glass'] as const) {
    for (const scheme of ['dark', 'light'] as const) {
      it(`${look} in a ${scheme} Garden (the computer in ${scheme === 'dark' ? 'Light' : 'Dark'} mode)`, async () => {
        const { page, png, shot } = await open(look, scheme)
        try {
          save(`k2-thread-${look}-${scheme}.png`, shot)
          const frame = page.frameLocator('#f')
          expect(await frame.locator('html').evaluate((el) => getComputedStyle(el).colorScheme)).toBe(scheme)
          const bubbles = frame.locator('.k2t-bubble')
          expect(await bubbles.count()).toBe(3)
          const fr = await page.locator('#f').boundingBox()
          const agent = await frame.locator('.k2t-bubble-agent').first().boundingBox()
          const me = await frame.locator('.k2t-bubble-me').first().boundingBox()
          if (!fr || !agent || !me) throw new Error('no layout')
          // Inside the left padding at mid height: the fill, no text.
          const a = pixel(png, agent.x + 5, agent.y + agent.height / 2)
          expect(near(a, hex(AGENT_FILL[look][scheme])), `agent bubble rgb(${a.join(',')})`).toBe(true)
          const m = pixel(png, me.x + me.width - 5, me.y + me.height / 2)
          expect(near(m, hex(ME_FILL[look][scheme])), `your bubble rgb(${m.join(',')})`).toBe(true)
          // The top-left corner pixel: cut (chamfer) or rounded (glass) shows
          // what is behind, never the bubble.
          const corner = pixel(png, agent.x + 0.5, agent.y + 0.5)
          expect(near(corner, hex(AGENT_FILL[look][scheme])), `corner rgb(${corner.join(',')}) is cut or round`).toBe(false)
          const shape = await frame.locator('.k2t-bubble-agent').first().evaluate((el) => ({
            clip: getComputedStyle(el, '::after').clipPath,
            radius: getComputedStyle(el).borderTopLeftRadius,
          }))
          if (look === 'chamfer') {
            expect(shape.clip).toMatch(/^polygon\(/)
            expect(shape.radius).toBe('0px')
          } else {
            expect(shape.radius).toBe('18px')
          }
          // The working tab and the pill-free bottom.
          expect(await frame.locator('.k2t-ear').getAttribute('data-tone')).toBe('working')
          expect(await frame.locator('.k2t-ear [data-count]').count()).toBe(3)
          expect(await frame.locator('.k2t-newer').isHidden()).toBe(true)
          expect(await frame.locator('.k2t-send').evaluate((el) => el.classList.contains('k2t-ready'))).toBe(true)
        } finally {
          await page.close()
        }
      })
    }
  }

  it('a narrow panel (under 480px) lets a message use the whole width but 12px', async () => {
    const { page } = await open('glass', 'dark', 340)
    try {
      const frame = page.frameLocator('#f')
      const msgs = await frame.locator('.k2t-msgs').boundingBox()
      const first = frame.locator('.k2t-msg').first()
      if (!msgs) throw new Error('no layout')
      // The container query's rule applies (WebKit reports it as written).
      expect(await first.evaluate((el) => getComputedStyle(el).maxWidth)).toBe('calc(100% - 12px)')
      // A long message really takes the narrow width less the inset.
      const box = await frame.locator('.k2t-msg').nth(0).boundingBox()
      if (!box) throw new Error('no layout')
      expect(Math.round(box.width)).toBeLessThanOrEqual(Math.round(msgs.width) - 12)
      expect(Math.round(box.width)).toBeGreaterThan(Math.round(msgs.width * 0.82))
    } finally {
      await page.close()
    }
  })
})

describe('k2-help@1 in WebKit', () => {
  for (const scheme of ['dark', 'light'] as const) {
    it(`the (i) tip opens in the Garden's ${scheme} colours`, async () => {
      const { page } = await open('chamfer', scheme)
      try {
        const frame = page.frameLocator('#f')
        await frame.locator('.k2h-btn').click()
        const tip = frame.locator('.k2h-tip')
        await expect.poll(async () => tip.isVisible()).toBe(true)
        const bg = await tip.evaluate((el) => getComputedStyle(el).backgroundColor)
        expect(bg).toBe(`rgb(${hex(BASIC[scheme]['--zen-surface-raised']).join(', ')})`)
        const shot = await page.screenshot({ clip: { x: 0, y: 0, width: 436, height: 160 } })
        save(`k2-help-tip-${scheme}.png`, shot)
        await page.keyboard.press('Escape')
        await expect.poll(async () => tip.isVisible()).toBe(false)
      } finally {
        await page.close()
      }
    })
  }
})
