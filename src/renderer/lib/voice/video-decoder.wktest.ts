// Video in voice rooms: the real receive path in headless WebKit (the engine of
// the desktop app's macOS webview). Real VP8 from WebKit's own `VideoEncoder`
// (the dev demo's synthetic source), framed as Sew's plugin frames it, through
// `SinkDecoder` and the real WebCodecs `VideoDecoder` onto a canvas. Pixels are
// read back. A keyframe request mid-stream must reach the source and the picture
// must keep coming.
//
//   bun run test:webkit   (npx playwright install webkit first)

import { resolve } from 'node:path'
import { build } from 'esbuild'
import { webkit, type Browser } from 'playwright'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

const here = resolve(__dirname)

/** The page's script: the shipped modules, bundled. */
async function bundle(): Promise<string> {
  const entry = `
    import { SinkDecoder, canvasSurface, probeDecode, webCodecsEnv } from './video-decoder'
    import { syntheticVp8Source } from './synthetic-video-source'
    window.runVideo = async () => {
      const env = webCodecsEnv()
      const probe = await probeDecode(env)
      const canvas = document.createElement('canvas')
      document.body.appendChild(canvas)
      const source = syntheticVp8Source({ label: 'Acme', color: '#d01010', width: 320, height: 180, fps: 30 })
      let keyframesAsked = 0
      const decoder = new SinkDecoder({
        env,
        surface: () => canvasSurface(canvas),
        requestKeyframe: () => { keyframesAsked += 1; source.keyframe() },
      })
      source.start({ sinkId: 1, track: 'TR_1', selfView: false }, (data) => decoder.push(data))
      const until = async (ok) => { for (let i = 0; i < 200 && !ok(); i++) await new Promise((r) => setTimeout(r, 25)) }
      await until(() => decoder.stats.decoded >= 5)
      const before = decoder.stats.decoded
      decoder.resume()
      await until(() => decoder.stats.decoded >= before + 5)
      source.stop()
      const px = canvas.getContext('2d').getImageData(300, 10, 1, 1).data
      decoder.close()
      return { probe, stats: decoder.stats, keyframesAsked, size: [canvas.width, canvas.height], px: Array.from(px) }
    }
  `
  const out = await build({
    stdin: { contents: entry, resolveDir: here, loader: 'ts' },
    bundle: true,
    write: false,
    format: 'iife',
    target: 'es2022',
  })
  return out.outputFiles[0].text
}

let browser: Browser

beforeAll(async () => {
  browser = await webkit.launch()
})

afterAll(async () => {
  await browser?.close()
})

describe('the WebCodecs receive path in WebKit', () => {
  it('decodes synthetic VP8 frames onto the canvas, and a keyframe request keeps it going', async () => {
    const page = await browser.newPage()
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(String(e)))
    await page.setContent('<!doctype html><html><body></body></html>')
    await page.addScriptTag({ content: await bundle() })
    const r = (await page.evaluate('window.runVideo()')) as {
      probe: string[]
      stats: { decoded: number; dropped: number }
      keyframesAsked: number
      size: [number, number]
      px: number[]
    }
    expect(errors).toEqual([])
    expect(r.probe).toContain('vp8')
    expect(r.size).toEqual([320, 180])
    expect(r.stats.decoded).toBeGreaterThanOrEqual(10)
    expect(r.keyframesAsked).toBe(1)
    // The card's red background (top right, clear of the label and the moving
    // square), after a lossy round trip.
    const [red, green, blue, alpha] = r.px
    expect(alpha).toBe(255)
    expect(red).toBeGreaterThan(150)
    expect(green).toBeLessThan(80)
    expect(blue).toBeLessThan(80)
    await page.close()
  })
})
