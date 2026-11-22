// The WebGL terminal painter's GPU lifecycle in real headless WebKit (the
// engine of the desktop app's macOS webview). Field report: WebKit's GPU
// process at 21 GB after 16 h. The shipped painter runs in a page and goes
// through the triggers a long session repeats: panes shown once and parked
// (their canvas stays in the document), tabs opened and closed, and font /
// zoom changes. WebKit itself must report every released context lost and
// every released canvas empty. The GPU process's resident size is sampled
// with `ps` around each phase and printed (a measurement, not a gate: RSS
// depends on the machine and on when WebKit collects).
//
//   bun run test:webkit   (npx playwright install webkit first)

import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { build, type Plugin } from 'esbuild'
import { webkit, type Browser } from 'playwright'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

const here = resolve(__dirname)
const renderer = resolve(__dirname, '../..')

/** `@/…` imports resolve to src/renderer, as in the app's Vite config. */
const atAlias: Plugin = {
  name: 'at-alias',
  setup(b) {
    b.onResolve({ filter: /^@\// }, async (args) =>
      b.resolve(`./${args.path.slice(2)}`, { resolveDir: renderer, kind: args.kind }),
    )
  },
}

async function bundle(): Promise<string> {
  const entry = `
    import { createWebglPainter } from './webglPainter'
    const run = (text) => ({ text, fg: null, bg: null, bold: false, italic: false, underline: false, inverse: false, dim: false, strikeout: false })
    // A wide glyph set so the atlas grows past its first page.
    const glyphs = []
    for (let cp = 0x4e00; glyphs.length < 3000; cp++) glyphs.push(String.fromCodePoint(cp))
    const frame = (cols, rows, salt) => ({
      snapshot: {
        cols, rows, scrollback: [],
        grid: Array.from({ length: rows }, (_, r) => {
          let s = ''
          for (let c = 0; c < cols / 2; c++) s += glyphs[(salt * 977 + r * cols + c) % glyphs.length]
          return [run(r % 3 ? s : 'ascii ┌──┐ ' + r)]
        }),
      },
      scrollPx: 0, selection: null,
      theme: { fg: 0xdddddd, bg: 0x101010, selection: 0x334455, textGamma: 1 },
    })
    const metrics = (fontSize, dpr) => ({ cssCellW: fontSize * 0.6, cssCellH: fontSize * 1.3, dpr, fontFamily: 'Menlo, monospace', fontSize })
    const fatals = []
    const painterOn = (canvas, fontSize = 13) => {
      const p = createWebglPainter()
      p.onFatal((r) => fatals.push(r))
      p.mount(canvas)
      p.setMetrics(metrics(fontSize, 2))
      p.render(frame(160, 50, fontSize))
      return p
    }
    const parked = []
    const pause = () => new Promise((r) => setTimeout(r, 50))
    window.gpu = {
      // Panes shown once then parked: the canvas stays in the document.
      async parkPanes(n) {
        for (let i = 0; i < n; i++) {
          const c = document.createElement('canvas')
          document.body.appendChild(c)
          const p = painterOn(c)
          p.dispose()
          parked.push(c)
          await pause()
        }
        const live = parked.filter((c) => { const gl = c.getContext('webgl2'); return gl && !gl.isContextLost() }).length
        const pixels = parked.reduce((n, c) => n + c.width * c.height, 0)
        return { live, pixels, fatals: fatals.slice(), atlasPages: document.querySelectorAll('[data-k2-atlas-page]').length }
      },
      // Tabs opened and closed: the canvas leaves the document.
      async openClose(n) {
        const closed = []
        for (let i = 0; i < n; i++) {
          const c = document.createElement('canvas')
          document.body.appendChild(c)
          painterOn(c).dispose()
          c.remove()
          closed.push(c)
          if (i % 10 === 0) await pause()
        }
        const live = closed.filter((c) => { const gl = c.getContext('webgl2'); return gl && !gl.isContextLost() }).length
        closed.length = 0
        return { live, fatals: fatals.slice(), atlasPages: document.querySelectorAll('[data-k2-atlas-page]').length }
      },
      // Font / zoom changes on one live pane.
      async fontChurn(n) {
        const c = document.createElement('canvas')
        document.body.appendChild(c)
        const p = painterOn(c)
        for (let i = 0; i < n; i++) {
          p.setMetrics(metrics(11 + (i % 9), i % 2 ? 2 : 1))
          p.render(frame(160, 50, i))
          if (i % 10 === 0) await pause()
        }
        const pages = document.querySelectorAll('[data-k2-atlas-page]').length
        p.dispose()
        c.remove()
        return { pagesWhileLive: pages, fatals: fatals.slice(), atlasPages: document.querySelectorAll('[data-k2-atlas-page]').length }
      },
    }
  `
  const out = await build({
    stdin: { contents: entry, resolveDir: here, loader: 'ts' },
    bundle: true,
    write: false,
    format: 'iife',
    target: 'es2022',
    plugins: [atAlias],
  })
  return out.outputFiles[0].text
}

/** Resident MB of this WebKit build's processes, by kind. */
function webkitRss(): Record<string, number> {
  const lines = execFileSync('ps', ['-axo', 'rss=,command='], { encoding: 'utf8' }).split('\n')
  const out: Record<string, number> = {}
  for (const line of lines) {
    if (!/ms-playwright\/webkit|WebKit\.framework/.test(line)) continue
    const m = line.trim().match(/^(\d+)\s+(.*)$/)
    if (!m) continue
    const cmd = m[2]
    const kind = /GPU/i.test(cmd) ? 'gpu' : /WebContent/i.test(cmd) ? 'web' : /Networking/i.test(cmd) ? 'net' : 'other'
    out[kind] = (out[kind] ?? 0) + Math.round(Number(m[1]) / 1024)
  }
  return out
}

let browser: Browser

beforeAll(async () => {
  browser = await webkit.launch()
})

afterAll(async () => {
  await browser?.close()
})

describe('the WebGL terminal painter in WebKit gives its GPU memory back', () => {
  it('parked panes, closed tabs and font changes leave no live context, buffer or atlas page', async () => {
    const page = await browser.newPage()
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(String(e)))
    await page.setContent('<!doctype html><html><body></body></html>')
    await page.addScriptTag({ content: await bundle() })
    const rss: Array<[string, Record<string, number>]> = [['start', webkitRss()]]

    const parked = (await page.evaluate('window.gpu.parkPanes(40)')) as {
      live: number
      pixels: number
      fatals: string[]
      atlasPages: number
    }
    rss.push(['after 40 parked panes', webkitRss()])
    const closed = (await page.evaluate('window.gpu.openClose(100)')) as { live: number; fatals: string[]; atlasPages: number }
    rss.push(['after 100 tab open/close', webkitRss()])
    const font = (await page.evaluate('window.gpu.fontChurn(100)')) as {
      pagesWhileLive: number
      fatals: string[]
      atlasPages: number
    }
    rss.push(['after 100 font changes', webkitRss()])
    // eslint-disable-next-line no-console
    console.log(
      '[gpu-probe] WebKit resident MB by process kind:\n' +
        rss.map(([at, r]) => `  ${at.padEnd(26)} ${JSON.stringify(r)}`).join('\n') +
        `\n  parked=${JSON.stringify(parked)} closed=${JSON.stringify(closed)} font=${JSON.stringify(font)}`,
    )

    expect(errors).toEqual([])
    expect(parked.fatals).toEqual([])
    expect(parked.live, 'parked panes still holding a WebGL context').toBe(0)
    expect(parked.pixels, 'parked panes still holding a drawing buffer').toBe(0)
    expect(parked.atlasPages).toBe(0)
    expect(closed.live, 'closed tabs still holding a WebGL context').toBe(0)
    expect(closed.atlasPages).toBe(0)
    expect(font.pagesWhileLive).toBe(1)
    expect(font.atlasPages).toBe(0)
    expect(font.fatals).toEqual([])
    await page.close()
  }, 180_000)
})
