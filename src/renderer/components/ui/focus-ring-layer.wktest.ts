// The shared keyboard-focus ring (data-k2-focus) in the real engine: the app's
// globals.css, compiled by Tailwind as the app build does, rendered in
// headless WebKit (the desktop webview's engine). An element's own absolute,
// fixed or sticky class wins over the ring's positioned-box default (that
// default sits in a layer under Tailwind's), an element with no position
// class still gets its box, and the ring still draws (its computed ::after),
// on an absolute element too, in Terminal and Glass. (Headless WebKit on the
// build Macs returns empty screenshots, so the pixels are checked in the
// Chromium shots instead.) focus-ring-layer.test.ts pins the CSS.
//
//   bun run test:webkit   (npx playwright install webkit first)

import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import { webkit, type Browser, type Page } from 'playwright'
import { compile } from 'tailwindcss'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

const require = createRequire(import.meta.url)
const renderer = resolve(__dirname, '../..')

/** The classes the page uses (the build's scanner would find them in the app). */
const CANDIDATES = ['absolute', 'fixed', 'sticky', 'relative', 'right-1.5', 'top-1', 'top-0', 'bottom-2', 'left-2', 'w-6', 'h-6', 'block']

async function appCss(): Promise<string> {
  const file = resolve(renderer, 'globals.css')
  const compiler = await compile(readFileSync(file, 'utf8'), {
    base: renderer,
    loadStylesheet: async (id: string, base: string) => {
      const path = id === 'tailwindcss' ? require.resolve('tailwindcss/index.css') : resolve(base, id)
      return { path, base: dirname(path), content: readFileSync(path, 'utf8') }
    },
  })
  // Fonts live next to globals.css; the page doesn't need them.
  return compiler.build(CANDIDATES).replace(/@font-face\s*\{[^}]*\}/g, '')
}

const LOOKS = [
  { style: 'terminal', palette: 'charcoal', scheme: 'dark' },
  { style: 'glass', palette: 'veil', scheme: 'light' },
] as const

function doc(css: string, look: (typeof LOOKS)[number]): string {
  return `<!doctype html><html data-style="${look.style}" data-palette="${look.palette}" data-scheme="${look.scheme}"><head><meta charset="utf-8"><style>${css}</style></head>
<body style="margin:0;background:var(--color-bg)">
<div id="field" style="position:relative;display:inline-flex;align-items:center;width:300px;height:40px;margin:20px;background:var(--color-bg-surface)">
  <input id="input" style="width:100%;height:40px;box-sizing:border-box;border:0">
  <button id="abs" data-k2-focus="" class="absolute right-1.5 w-6 h-6" style="border:0;background:var(--color-bg-surface)"></button>
</div>
<div id="flow" style="position:relative;width:300px;margin:20px">
  <button id="plain" data-k2-focus="" class="block" style="width:200px;height:32px;border:0;background:var(--color-bg-surface)">Row</button>
  <span id="kid-host"><button id="fixed" data-k2-focus="" class="fixed bottom-2 left-2" style="width:40px;height:20px"></button></span>
  <button id="sticky" data-k2-focus="" class="sticky top-0" style="width:40px;height:20px"></button>
  <button id="rel" data-k2-focus="" class="relative" style="width:40px;height:20px"></button>
</div>
<div data-zen-root=""><button id="garden" data-k2-focus="" style="width:40px;height:20px"></button></div>
<div id="probe" style="width:20px;height:20px;background:var(--color-accent-text)"></div>
</body></html>`
}

const position = (page: Page, id: string): Promise<string> => page.evaluate((x) => getComputedStyle(document.getElementById(x)!).position, id)

/** Keyboard focus (so :focus-visible matches). WebKit's Tab skips buttons
 *  unless "Tab to all controls" is on, so: a key press, then focus moved by
 *  script from a focused text field (both keep :focus-visible). */
async function keyboardFocus(page: Page, id: string): Promise<void> {
  await page.evaluate(() => (document.getElementById('input') as HTMLElement).focus())
  await page.keyboard.press('Shift')
  await page.evaluate((x) => (document.getElementById(x) as HTMLElement).focus(), id)
  expect(await page.evaluate(() => document.activeElement?.id)).toBe(id)
  expect(await page.evaluate((x) => document.getElementById(x)!.matches(':focus-visible'), id)).toBe(true)
}

let browser: Browser
let css: string
beforeAll(async () => {
  css = await appCss()
  browser = await webkit.launch()
})
afterAll(async () => {
  await browser?.close()
})

describe.each(LOOKS)('data-k2-focus in WebKit: $style', (look) => {
  it('a Tailwind position class wins over the ring\'s positioned box', async () => {
    const page = await browser.newPage({ viewport: { width: 500, height: 400 } })
    await page.setContent(doc(css, look))
    expect(await position(page, 'abs')).toBe('absolute')
    expect(await position(page, 'fixed')).toBe('fixed')
    expect(await position(page, 'sticky')).toBe('sticky')
    expect(await position(page, 'rel')).toBe('relative')
    // No position class: the ring's own positioned box.
    expect(await position(page, 'plain')).toBe('relative')
    // Gardens keep their own look.
    expect(await position(page, 'garden')).toBe('static')
    // The absolute button sits over the field's right end, not after it.
    const [field, abs] = await Promise.all(['field', 'abs'].map((id) => page.locator(`#${id}`).boundingBox()))
    expect(abs!.x + abs!.width).toBeLessThanOrEqual(field!.x + field!.width)
    expect(abs!.x + abs!.width).toBeGreaterThan(field!.x + field!.width - 10)
    expect(abs!.y).toBeGreaterThanOrEqual(field!.y)
    expect(abs!.y + abs!.height).toBeLessThanOrEqual(field!.y + field!.height)
    await page.close()
  })

  it('the ring still draws, on a plain row and on an absolute button', async () => {
    const page = await browser.newPage({ viewport: { width: 500, height: 400 } })
    await page.setContent(doc(css, look))
    // The accent as text, as this engine computes it.
    const accent = await page.evaluate(() => getComputedStyle(document.getElementById('probe')!).backgroundColor)
    for (const id of ['plain', 'abs']) {
      const before = await page.locator(`#${id}`).boundingBox()
      const ring = (): Promise<{ content: string; position: string; inset: string[]; z: string; shadow: string; fill: string; clip: string; outline: string }> =>
        page.evaluate((x) => {
          const el = document.getElementById(x)!
          const s = getComputedStyle(el, '::after')
          return {
            content: s.content,
            position: s.position,
            inset: [s.top, s.right, s.bottom, s.left],
            z: s.zIndex,
            shadow: s.boxShadow,
            fill: s.backgroundColor,
            clip: s.clipPath,
            outline: getComputedStyle(el).outlineStyle,
          }
        }, id)
      expect(['none', 'normal'], `${id} at rest has no ring`).toContain((await ring()).content)
      await keyboardFocus(page, id)
      const r = await ring()
      expect(['none', 'normal'], id).not.toContain(r.content)
      expect(r.position, id).toBe('absolute')
      expect(r.inset, id).toEqual(['0px', '0px', '0px', '0px'])
      expect(r.z, id).toBe('1')
      expect(r.outline, id).toBe('none')
      if (look.style === 'terminal') {
        // The 2px band with the control's cuts: an accent fill clipped to a ring.
        expect(r.fill, id).toBe(accent)
        expect(r.clip, id).toMatch(/^polygon\(evenodd/)
      } else {
        expect(r.shadow, id).toContain(accent)
        expect(r.shadow, id).toMatch(/0px 0px 0px 2px inset|inset 0px 0px 0px 2px/)
      }
      // Focus moves nothing.
      expect(await page.locator(`#${id}`).boundingBox()).toEqual(before)
    }
    await page.close()
  })
})
