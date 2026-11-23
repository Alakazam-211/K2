// What showing a WebGL terminal costs, in real headless WebKit (the engine
// of the desktop app's webview). Owner report 0.46.17: switching between
// agents isn't snappy. Since a4b0b3b19c a hidden pane gives its context
// back, so every show builds a painter: a WebGL2 context, two linked
// programs, a pixel-readback sanity probe, a glyph atlas warmed up and
// uploaded, then the first frame. This prints each step's median over a
// run of shows and checks that a show reusing the shared surface and
// atlas (glSurfacePool, atlasCache) skips them.
//
//   bun run test:webkit   (npx playwright install webkit first)
//
// Timings are printed, not gated (they depend on the machine). The gate is
// the reuse itself and the live-context count: never more than the panes on
// screen plus the one spare.

import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { build, type Plugin } from 'esbuild'
import { webkit, type Browser } from 'playwright'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

const here = resolve(__dirname)
const renderer = resolve(__dirname, '../..')

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
    import { createWebgl2Backend } from './glBackend'
    import { GlyphAtlas } from './glyphAtlas'
    import { liveSurfaceCount, releaseAllSurfaces } from './glSurfacePool'
    import { liveAtlasCount, releaseAllAtlases } from './atlasCache'
    const run = (text) => ({ text, fg: null, bg: null, bold: false, italic: false, underline: false, inverse: false, dim: false, strikeout: false })
    const frame = (cols, rows, salt) => ({
      snapshot: {
        cols, rows, scrollback: [],
        grid: Array.from({ length: rows }, (_, r) => [run(('agent ' + salt + ' line ' + r + ' ┌──┐ ✓ the quick brown fox ').repeat(3).slice(0, cols))]),
      },
      scrollPx: 0, selection: null,
      theme: { fg: 0xdddddd, bg: 0x101010, selection: 0x334455, textGamma: 1 },
    })
    const metrics = { cssCellW: 7.8, cssCellH: 16.9, dpr: 2, fontFamily: 'Menlo, monospace', fontSize: 13 }
    const median = (xs) => { const s = xs.slice().sort((a, b) => a - b); return s[s.length >> 1] }
    const pause = () => new Promise((r) => setTimeout(r, 30))
    const finish = (c) => { const gl = c.getContext('webgl2'); gl && gl.finish() }
    window.bench = {
      // The steps of a fresh show, timed one by one (fresh canvas each time).
      async steps(n) {
        const ctx = [], link = [], atlas = [], first = [], total = []
        for (let i = 0; i < n; i++) {
          const c = document.createElement('canvas')
          document.body.appendChild(c)
          let t = performance.now()
          const gl = c.getContext('webgl2', { alpha: false, antialias: false, premultipliedAlpha: false, preserveDrawingBuffer: false, powerPreference: 'high-performance' })
          ctx.push(performance.now() - t)
          t = performance.now()
          const be = createWebgl2Backend(c)
          be.resize(4, 4); be.beginFrame(0x3a7b19); be.readPixel(1, 1)
          link.push(performance.now() - t)
          t = performance.now()
          const a = new GlyphAtlas({ deviceCellW: 15, deviceCellH: 34, fontFamily: 'Menlo, monospace', fontDevicePx: 26 })
          a.warmUp()
          atlas.push(performance.now() - t)
          a.dispose()
          be.dispose({ release: true })
          c.remove()
          await pause()
        }
        return { ctx: median(ctx), linkAndProbe: median(link), atlasWarm: median(atlas) }
      },
      // A whole show through the painter: build (or reuse), first frame.
      async shows(n, reuse) {
        const times = [], lives = []
        let prev = null
        // The switch: the pane on screen hides (dispose) and the next one
        // shows (mount, metrics, first frame), timed together.
        for (let i = 0; i < n; i++) {
          // reuse: TerminalPane's host mode; else the 0.46.17 path (a
          // fresh canvas per show, its own atlas).
          const target = reuse ? document.createElement('div') : document.createElement('canvas')
          document.body.appendChild(target)
          const t = performance.now()
          if (prev) { prev.p.dispose(); prev.target.remove() }
          const p = createWebglPainter()
          p.onFatal((r) => { throw new Error(r) })
          p.mount(target)
          p.setMetrics(metrics)
          p.render(frame(160, 50, i))
          finish(reuse ? target.querySelector('canvas') : target)
          times.push(performance.now() - t)
          lives.push(liveSurfaceCount() + (reuse ? 0 : 1))
          prev = { p, target }
          await pause()
        }
        if (prev) { prev.p.dispose(); prev.target.remove() }
        // Nothing on screen: what is still held (the spare, the idle atlas).
        const heldAfterHide = liveSurfaceCount() + liveAtlasCount()
        releaseAllSurfaces()
        releaseAllAtlases()
        return { median: median(times), max: Math.max(...times), maxLiveSurfaces: Math.max(...lives), heldAfterHide, liveAfterRelease: liveSurfaceCount() + liveAtlasCount() }
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

/** Resident MB of this WebKit build's processes, by kind (printed, never
 *  asserted; same sampler as webglPainter.gpu.wktest). */
function webkitRss(): Record<string, number | string> {
  let lines: string[]
  try {
    lines = execFileSync('ps', ['-axo', 'rss=,command='], { encoding: 'utf8' }).split('\n')
  } catch (e) {
    return { unavailable: e instanceof Error ? e.message : String(e) }
  }
  const out: Record<string, number> = {}
  for (const line of lines) {
    if (!/ms-playwright\/webkit|WebKit\.framework/.test(line)) continue
    const m = line.trim().match(/^(\d+)\s+(.*)$/)
    if (!m) continue
    const kind = /GPU/i.test(m[2]) ? 'gpu' : /WebContent/i.test(m[2]) ? 'web' : /Networking/i.test(m[2]) ? 'net' : 'other'
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

describe('showing a WebGL terminal in WebKit', () => {
  it('reusing the spare surface and the shared atlas makes a show cheap', async () => {
    const page = await browser.newPage()
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(String(e)))
    await page.setContent('<!doctype html><html><body></body></html>')
    await page.addScriptTag({ content: await bundle() })
    const steps = (await page.evaluate('window.bench.steps(15)')) as Record<string, number>
    const rss: Array<[string, Record<string, number | string>]> = [['start', webkitRss()]]
    const fresh = (await page.evaluate('window.bench.shows(40, false)')) as Record<string, number>
    rss.push(['after 40 fresh shows', webkitRss()])
    const reused = (await page.evaluate('window.bench.shows(40, true)')) as Record<string, number>
    rss.push(['after 40 reused shows', webkitRss()])
    const r = (v: number) => Math.round(v * 10) / 10
    // eslint-disable-next-line no-console
    console.log(
      `[show-cost] fresh-show steps (median ms): context=${r(steps.ctx)} programs+probe=${r(steps.linkAndProbe)} atlas warm-up=${r(steps.atlasWarm)}\n` +
        `[show-cost] whole show to first frame: fresh median=${r(fresh.median)} max=${r(fresh.max)} | reused median=${r(reused.median)} max=${r(reused.max)} (live surfaces max ${reused.maxLiveSurfaces}, held with nothing on screen ${reused.heldAfterHide})\n` +
        rss.map(([at, v]) => `[show-cost] WebKit resident MB ${at.padEnd(22)} ${JSON.stringify(v)}`).join('\n'),
    )
    expect(errors).toEqual([])
    // One pane on screen at a time: its surface, and at most one spare.
    expect(reused.maxLiveSurfaces).toBeLessThanOrEqual(2)
    expect(reused.median).toBeLessThan(fresh.median)
    // Released means released: no context and no atlas left.
    expect(reused.liveAfterRelease).toBe(0)
    await page.close()
  }, 180_000)
})
